use serde::{Deserialize, Serialize};

pub const USAGE_ANALYTICS_VERSION: &str = "overview-v2";
pub const USAGE_ANALYTICS_MAX_RANGE_MS: u64 = 366 * 24 * 60 * 60 * 1000;
pub const USAGE_DASHBOARD_CHART_ROW_LIMIT: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageDashboardAnalyticsQuery {
    pub timezone: String,
}

impl UsageDashboardAnalyticsQuery {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        self.timezone
            .parse::<chrono_tz::Tz>()
            .map(|_| ())
            .map_err(|_| crate::DataLayerError::InvalidInput("invalid analytics timezone".into()))
    }

    pub fn today_start(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<chrono::DateTime<chrono::Utc>, crate::DataLayerError> {
        self.validate()?;
        let timezone = self.timezone.parse::<chrono_tz::Tz>().expect("validated");
        local_day_start(timezone, now.with_timezone(&timezone).date_naive()).ok_or_else(|| {
            crate::DataLayerError::InvalidInput("reporting day boundary is unavailable".into())
        })
    }
}

fn local_day_start(
    timezone: chrono_tz::Tz,
    day: chrono::NaiveDate,
) -> Option<chrono::DateTime<chrono::Utc>> {
    use chrono::TimeZone;
    let midnight = day.and_hms_opt(0, 0, 0)?;
    // IANA transitions can skip midnight or an entire local calendar day.
    (0..1440)
        .find_map(|minutes| {
            timezone
                .from_local_datetime(&(midnight + chrono::Duration::minutes(minutes)))
                .earliest()
        })
        .map(|value| value.with_timezone(&chrono::Utc))
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StoredUsageDashboardAnalytics {
    pub today: StoredUsageAnalytics,
    // Lifetime card totals and coverage only; historical diagnostics are not computed.
    pub total: StoredUsageAnalytics,
    pub today_from: String,
    pub total_from: Option<String>,
    pub to: String,
    // False means known lost history; None means installation-wide retention is unproven.
    pub history_complete: Option<bool>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageAnalyticsView {
    #[default]
    Summary,
    Timeseries,
    Breakdown,
    Users,
    Consumption,
    Performance,
    DashboardCharts,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageAnalyticsGroupBy {
    #[default]
    Model,
    Provider,
    ApiKey,
    Attribution,
    ApiFormat,
    RequestType,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageAnalyticsGranularity {
    Hour,
    #[default]
    Day,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageAnalyticsSort {
    #[default]
    Requests,
    BillableAmount,
    LastUsed,
    Username,
    Tokens,
    ActiveDays,
    StartedAt,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageAnalyticsQuery {
    pub from_unix_ms: u64,
    pub to_unix_ms: u64,
    pub timezone: String,
    pub view: UsageAnalyticsView,
    pub group_by: UsageAnalyticsGroupBy,
    pub granularity: UsageAnalyticsGranularity,
    pub actor_user_id: Option<String>,
    pub credential_owner_id: Option<String>,
    pub attribution_kind: Option<String>,
    pub api_key_id: Option<String>,
    pub model: Option<String>,
    pub provider_id: Option<String>,
    pub api_format: Option<String>,
    pub endpoint_kind: Option<String>,
    pub request_type: Option<String>,
    pub status: Option<String>,
    pub is_stream: Option<bool>,
    pub has_format_conversion: Option<bool>,
    pub slow_threshold_ms: Option<u64>,
    pub search: Option<String>,
    pub user_is_active: Option<bool>,
    pub has_usage: Option<bool>,
    pub sort: UsageAnalyticsSort,
    pub descending: bool,
    pub limit: u32,
    pub offset: u64,
    #[serde(default)]
    pub payment_limit: Option<u32>,
    #[serde(default)]
    pub payment_offset: Option<u64>,
}

impl UsageAnalyticsQuery {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        if self.from_unix_ms >= self.to_unix_ms
            || self.to_unix_ms - self.from_unix_ms > USAGE_ANALYTICS_MAX_RANGE_MS
            || self.to_unix_ms > 253_402_300_799_000
        {
            return Err(crate::DataLayerError::InvalidInput(
                "analytics range must be nonempty and at most 366 days".into(),
            ));
        }
        if self.view == UsageAnalyticsView::DashboardCharts
            && self.granularity == UsageAnalyticsGranularity::Hour
            && self.to_unix_ms - self.from_unix_ms > 31 * 24 * 60 * 60 * 1000
        {
            return Err(crate::DataLayerError::InvalidInput(
                "hourly dashboard charts are limited to 31 days".into(),
            ));
        }
        if self.timezone.parse::<chrono_tz::Tz>().is_err() {
            return Err(crate::DataLayerError::InvalidInput(
                "invalid analytics timezone".into(),
            ));
        }
        if self.limit == 0 || self.limit > 10_001 || self.offset > i64::MAX as u64 {
            return Err(crate::DataLayerError::InvalidInput(
                "invalid analytics pagination".into(),
            ));
        }
        if self
            .payment_limit
            .is_some_and(|value| value == 0 || value > 100)
            || self
                .payment_offset
                .is_some_and(|value| value > i64::MAX as u64)
            || (self.view != UsageAnalyticsView::Users
                && (self.payment_limit.is_some() || self.payment_offset.is_some()))
        {
            return Err(crate::DataLayerError::InvalidInput(
                "invalid user payment pagination".into(),
            ));
        }
        if self
            .attribution_kind
            .as_deref()
            .is_some_and(|kind| !matches!(kind, "employee" | "standalone" | "unknown"))
        {
            return Err(crate::DataLayerError::InvalidInput(
                "invalid attribution kind".into(),
            ));
        }
        Ok(())
    }
}

/// Raw domain metrics. Amounts are per-request normalized decimal sums, never floats.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UsageAnalyticsMetrics {
    pub request_count: u64,
    // Populated for dashboard chart buckets; distinct counts are not additive.
    pub unique_providers: Option<u64>,
    pub successful_request_count: u64,
    pub failed_request_count: u64,
    pub cancelled_request_count: u64,
    pub in_flight_request_count: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_pricing_available_count: u64,
    pub cache_read_cost_amount: Option<String>,
    pub cache_creation_cost_amount: Option<String>,
    pub cache_estimated_full_cost_amount: Option<String>,
    pub usage_active_users: u64,
    pub enabled_users: u64,
    pub usage_available_count: u64,
    pub reported_usage_count: u64,
    pub estimated_usage_count: u64,
    pub mixed_usage_count: u64,
    pub unknown_usage_count: u64,
    pub pricing_available_count: u64,
    pub settled_count: u64,
    pub allocation_available_count: u64,
    pub trusted_attribution_count: u64,
    pub classified_failure_count: u64,
    pub latency_sample_count: u64,
    pub slow_request_count: u64,
    pub latency_sum_ms: f64,
    pub latency_p50_ms: Option<f64>,
    pub latency_p95_ms: Option<f64>,
    pub latency_p90_ms: Option<f64>,
    pub latency_p99_ms: Option<f64>,
    pub first_byte_sample_count: u64,
    pub first_byte_sum_ms: f64,
    pub first_byte_p90_ms: Option<f64>,
    pub first_byte_p99_ms: Option<f64>,
    pub output_tps_sample_count: u64,
    pub output_tps_sum: f64,
    pub rated_amount: Option<String>,
    pub billable_amount: Option<String>,
    pub quota_covered_amount: Option<String>,
    pub wallet_consumed_amount: Option<String>,
    pub wallet_debit_amount: Option<String>,
    pub wallet_recharge_debit_amount: Option<String>,
    pub wallet_gift_debit_amount: Option<String>,
    pub wallet_overdraft_amount: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageAnalyticsRow {
    pub id: Option<String>,
    pub label: Option<String>,
    pub bucket_start: Option<String>,
    pub metrics: UsageAnalyticsMetrics,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageAnalyticsUser {
    pub user_id: String,
    pub username: String,
    pub email: Option<String>,
    pub is_active: bool,
    pub last_used_at: Option<String>,
    pub active_days: u64,
    pub metrics: UsageAnalyticsMetrics,
    #[serde(default)]
    pub finance: Option<UsageAnalyticsUserFinance>,
}

/// Current balances and gross credited orders in the requested time range.
/// Amounts are USD decimals. Gift-code/admin-grant orders and plan purchases
/// remain separate from wallet recharges; refunds are not assigned to the
/// original credit period.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageAnalyticsUserFinance {
    pub wallet_balance: Option<String>,
    pub recharge_balance: Option<String>,
    pub gift_balance: Option<String>,
    pub recharge_amount: Option<String>,
    pub recharge_count: u64,
    pub plan_purchase_amount: Option<String>,
    pub plan_purchase_count: u64,
    pub gift_credit_amount: Option<String>,
    pub gift_credit_count: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageAnalyticsUserPayment {
    pub id: String,
    pub order_no: String,
    pub kind: String,
    pub amount: String,
    pub payment_method: String,
    pub credited_at: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageAnalyticsUserPayments {
    pub items: Vec<UsageAnalyticsUserPayment>,
    pub total: u64,
    pub limit: u32,
    pub offset: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageAnalyticsUserSummary {
    pub user_count: u64,
    pub active_user_count: u64,
    pub metrics: UsageAnalyticsMetrics,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageAnalyticsConsumption {
    pub id: String,
    pub request_id: String,
    pub started_at: String,
    pub user_id: Option<String>,
    pub credential_owner_id: Option<String>,
    pub model: String,
    pub provider: Option<String>,
    pub provider_id: Option<String>,
    pub api_key_id: Option<String>,
    pub status: String,
    pub settlement_status: String,
    pub attribution_kind: String,
    pub attribution_source: String,
    pub rated_amount: Option<String>,
    pub billable_amount: Option<String>,
    pub quota_covered_amount: Option<String>,
    pub wallet_consumed_amount: Option<String>,
    pub wallet_debit_amount: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageAnalyticsAllocation {
    pub request_id: String,
    pub quota_covered_amount: Option<String>,
    pub wallet_consumed_amount: Option<String>,
    pub wallet_debit_amount: Option<String>,
    pub wallet_recharge_debit_amount: Option<String>,
    pub wallet_gift_debit_amount: Option<String>,
    pub wallet_overdraft_amount: Option<String>,
    pub cache_read_cost_amount: Option<String>,
    pub cache_creation_cost_amount: Option<String>,
    pub cache_estimated_full_cost_amount: Option<String>,
    pub complete: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StoredUsageAnalytics {
    pub summary: UsageAnalyticsMetrics,
    pub rows: Vec<UsageAnalyticsRow>,
    pub users: Vec<UsageAnalyticsUser>,
    #[serde(default)]
    pub user_summary: Option<UsageAnalyticsUserSummary>,
    #[serde(default)]
    pub user_finance_summary: Option<UsageAnalyticsUserFinance>,
    #[serde(default)]
    pub user_payments: Option<UsageAnalyticsUserPayments>,
    pub consumption: Vec<UsageAnalyticsConsumption>,
    pub provider_rows: Vec<UsageAnalyticsRow>,
    pub provider_timeline_rows: Vec<UsageAnalyticsRow>,
    pub model_rows: Vec<UsageAnalyticsRow>,
    pub errors: Vec<UsageAnalyticsErrorCount>,
    pub total: u64,
    pub read_revision: String,
    pub generated_at: String,
    pub data_through: Option<String>,
    pub unrecoverable_bucket_count: u64,
    pub coverage: UsageAnalyticsProjectionCoverage,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageAnalyticsProjectionCoverage {
    pub projection_from: Option<String>,
    pub projection_through: Option<String>,
    pub dirty_bucket_count: u64,
    pub missing_bucket_count: u64,
    pub read_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageAnalyticsErrorCount {
    pub reason: String,
    pub count: u64,
}

pub fn fill_usage_analytics_timeseries(
    query: &UsageAnalyticsQuery,
    rows: &mut Vec<UsageAnalyticsRow>,
) {
    use chrono::{Timelike, Utc};
    let timezone = query
        .timezone
        .parse::<chrono_tz::Tz>()
        .expect("validated timezone");
    let from = chrono::DateTime::<Utc>::from_timestamp_millis(query.from_unix_ms as i64)
        .expect("validated timestamp");
    let mut bucket = match query.granularity {
        UsageAnalyticsGranularity::Hour => from
            .with_minute(0)
            .and_then(|value| value.with_second(0))
            .and_then(|value| value.with_nanosecond(0))
            .expect("hour"),
        UsageAnalyticsGranularity::Day => {
            let day = from.with_timezone(&timezone).date_naive();
            let Some(value) = local_day_start(timezone, day) else {
                return;
            };
            value.with_timezone(&Utc)
        }
    };
    let mut existing = std::mem::take(rows)
        .into_iter()
        .filter_map(|row| {
            let start = row
                .bucket_start
                .as_ref()
                .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())?
                .timestamp_millis();
            Some((start, row))
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    while bucket.timestamp_millis() < query.to_unix_ms as i64 {
        let start = bucket.to_rfc3339();
        rows.push(
            existing
                .remove(&bucket.timestamp_millis())
                .unwrap_or_else(|| UsageAnalyticsRow {
                    id: Some(start.clone()),
                    label: Some(start.clone()),
                    bucket_start: Some(start),
                    metrics: UsageAnalyticsMetrics {
                        unique_providers: Some(0),
                        rated_amount: Some("0.00000000".into()),
                        billable_amount: Some("0.00000000".into()),
                        ..Default::default()
                    },
                }),
        );
        bucket = match query.granularity {
            UsageAnalyticsGranularity::Hour => bucket + chrono::Duration::hours(1),
            UsageAnalyticsGranularity::Day => {
                let mut day = bucket.with_timezone(&timezone).date_naive();
                loop {
                    let Some(next) = day.succ_opt() else {
                        return;
                    };
                    day = next;
                    if let Some(value) = local_day_start(timezone, day) {
                        break value;
                    }
                }
            }
        };
    }
}
