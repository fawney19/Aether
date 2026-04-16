use aether_data::repository::auth::StoredAuthApiKeySnapshot;
use aether_data_contracts::repository::{
    provider_catalog::StoredProviderCatalogProvider,
    usage::{StoredRequestUsageAudit, StoredUsageLeaderboardSummary, StoredUsageTimeSeriesBucket},
};
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use chrono::{Datelike, Utc};
use serde_json::json;
use url::form_urlencoded;

pub const MIN_PERCENTILE_SAMPLES: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdminStatsComparisonType {
    Period,
    Year,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdminStatsGranularity {
    Hour,
    Day,
    Week,
    Month,
}

#[derive(Clone, Debug)]
pub struct AdminStatsTimeRange {
    pub start_date: chrono::NaiveDate,
    pub end_date: chrono::NaiveDate,
    pub tz_offset_minutes: i32,
}

#[derive(Clone, Debug, Default)]
pub struct AdminStatsUsageFilter {
    pub user_id: Option<String>,
    pub provider_name: Option<String>,
    pub model: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct AdminStatsAggregate {
    pub total_requests: u64,
    pub total_tokens: u64,
    pub total_cost: f64,
    pub actual_total_cost: f64,
    pub total_response_time_ms: f64,
    pub error_requests: u64,
}

#[derive(Clone, Debug)]
pub struct AdminStatsForecastPoint {
    pub date: chrono::NaiveDate,
    pub total_cost: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdminStatsLeaderboardMetric {
    Requests,
    Tokens,
    Cost,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdminStatsSortOrder {
    Asc,
    Desc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdminStatsLeaderboardNameMode {
    Id,
    Name,
}

#[derive(Clone, Debug)]
pub struct AdminStatsLeaderboardItem {
    pub id: String,
    pub name: String,
    pub requests: u64,
    pub tokens: u64,
    pub cost: f64,
}

#[derive(Clone, Debug)]
pub struct AdminStatsUserMetadata {
    pub name: String,
    pub role: String,
    pub is_active: bool,
    pub is_deleted: bool,
}

#[derive(Clone, Debug, Default)]
pub struct AdminStatsTimeSeriesBucket {
    pub total_requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub total_cost: f64,
    pub total_response_time_ms: f64,
}

fn query_param_value(query: Option<&str>, key: &str) -> Option<String> {
    let query = query?;
    for (entry_key, value) in form_urlencoded::parse(query.as_bytes()) {
        if entry_key == key {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

pub fn parse_tz_offset_minutes(query: Option<&str>) -> Result<i32, String> {
    query_param_value(query, "tz_offset_minutes")
        .map(|value| {
            value
                .parse::<i32>()
                .map_err(|_| "tz_offset_minutes must be a valid integer".to_string())
        })
        .transpose()
        .map(|value| value.unwrap_or(0))
}

pub fn parse_naive_date(field: &str, value: &str) -> Result<chrono::NaiveDate, String> {
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| format!("{field} must be a valid date in YYYY-MM-DD format"))
}

pub fn parse_bounded_u32(field: &str, value: &str, min: u32, max: u32) -> Result<u32, String> {
    let parsed = value
        .parse::<u32>()
        .map_err(|_| format!("{field} must be a valid integer"))?;
    if parsed < min || parsed > max {
        return Err(format!("{field} must be between {min} and {max}"));
    }
    Ok(parsed)
}

pub fn parse_nonnegative_usize(field: &str, value: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|_| format!("{field} must be a valid integer"))
}

pub fn admin_usage_default_days() -> usize {
    match std::env::var("ADMIN_USAGE_DEFAULT_DAYS") {
        Ok(value) => value.parse::<usize>().ok().unwrap_or(0),
        Err(_) => match std::env::var("ENVIRONMENT") {
            Ok(value) if !matches!(value.as_str(), "development" | "test" | "testing") => 30,
            _ => 0,
        },
    }
}

pub fn user_today(tz_offset_minutes: i32) -> chrono::NaiveDate {
    (Utc::now() + chrono::Duration::minutes(i64::from(tz_offset_minutes))).date_naive()
}

pub fn resolve_preset_dates(
    preset: &str,
    tz_offset_minutes: i32,
) -> Result<(chrono::NaiveDate, chrono::NaiveDate), String> {
    let user_today = user_today(tz_offset_minutes);
    match preset {
        "today" => Ok((user_today, user_today)),
        "yesterday" => {
            let value = user_today
                .checked_sub_signed(chrono::Duration::days(1))
                .unwrap_or(user_today);
            Ok((value, value))
        }
        "last7days" => Ok((
            user_today
                .checked_sub_signed(chrono::Duration::days(6))
                .unwrap_or(user_today),
            user_today,
        )),
        "last30days" => Ok((
            user_today
                .checked_sub_signed(chrono::Duration::days(29))
                .unwrap_or(user_today),
            user_today,
        )),
        "last90days" => Ok((
            user_today
                .checked_sub_signed(chrono::Duration::days(89))
                .unwrap_or(user_today),
            user_today,
        )),
        "this_week" => {
            let week_start = user_today
                .checked_sub_signed(chrono::Duration::days(i64::from(
                    user_today.weekday().num_days_from_monday(),
                )))
                .unwrap_or(user_today);
            Ok((week_start, user_today))
        }
        "last_week" => {
            let this_week_start = user_today
                .checked_sub_signed(chrono::Duration::days(i64::from(
                    user_today.weekday().num_days_from_monday(),
                )))
                .unwrap_or(user_today);
            let last_week_end = this_week_start
                .checked_sub_signed(chrono::Duration::days(1))
                .unwrap_or(this_week_start);
            let last_week_start = last_week_end
                .checked_sub_signed(chrono::Duration::days(6))
                .unwrap_or(last_week_end);
            Ok((last_week_start, last_week_end))
        }
        "this_month" => {
            let start = user_today.with_day(1).unwrap_or(user_today);
            Ok((start, user_today))
        }
        "last_month" => {
            let first_of_this_month = user_today.with_day(1).unwrap_or(user_today);
            let last_month_end = first_of_this_month
                .checked_sub_signed(chrono::Duration::days(1))
                .unwrap_or(first_of_this_month);
            let last_month_start = last_month_end.with_day(1).unwrap_or(last_month_end);
            Ok((last_month_start, last_month_end))
        }
        "this_year" => {
            let start =
                chrono::NaiveDate::from_ymd_opt(user_today.year(), 1, 1).unwrap_or(user_today);
            Ok((start, user_today))
        }
        _ => Err("Invalid preset".to_string()),
    }
}

pub fn build_time_range_from_days(
    days: u32,
    tz_offset_minutes: i32,
) -> Result<AdminStatsTimeRange, String> {
    let end_date = user_today(tz_offset_minutes);
    let start_date = end_date
        .checked_sub_signed(chrono::Duration::days(i64::from(days.saturating_sub(1))))
        .unwrap_or(end_date);
    Ok(AdminStatsTimeRange {
        start_date,
        end_date,
        tz_offset_minutes,
    })
}

pub fn build_comparison_range(
    current: &AdminStatsTimeRange,
    comparison_type: AdminStatsComparisonType,
) -> Result<AdminStatsTimeRange, String> {
    let comparison = match comparison_type {
        AdminStatsComparisonType::Period => {
            let days = (current.end_date - current.start_date).num_days() + 1;
            let comparison_end = current
                .start_date
                .checked_sub_signed(chrono::Duration::days(1))
                .ok_or_else(|| "comparison range underflow".to_string())?;
            let comparison_start = comparison_end
                .checked_sub_signed(chrono::Duration::days(days - 1))
                .ok_or_else(|| "comparison range underflow".to_string())?;
            (comparison_start, comparison_end)
        }
        AdminStatsComparisonType::Year => (
            safe_year_shift(current.start_date),
            safe_year_shift(current.end_date),
        ),
    };

    Ok(AdminStatsTimeRange {
        start_date: comparison.0,
        end_date: comparison.1,
        tz_offset_minutes: current.tz_offset_minutes,
    })
}

fn safe_year_shift(value: chrono::NaiveDate) -> chrono::NaiveDate {
    value
        .with_year(value.year() - 1)
        .or_else(|| chrono::NaiveDate::from_ymd_opt(value.year() - 1, value.month(), 28))
        .unwrap_or(value)
}

impl AdminStatsAggregate {
    pub fn avg_response_time_ms(&self) -> f64 {
        if self.total_requests == 0 {
            0.0
        } else {
            self.total_response_time_ms / self.total_requests as f64
        }
    }
}

impl AdminStatsGranularity {
    pub fn parse(query: Option<&str>) -> Result<Self, String> {
        match query_param_value(query, "granularity").as_deref() {
            None | Some("day") => Ok(Self::Day),
            Some("hour") => Ok(Self::Hour),
            Some("week") => Ok(Self::Week),
            Some("month") => Ok(Self::Month),
            Some(_) => Err("granularity must be one of: hour, day, week, month".to_string()),
        }
    }
}

impl AdminStatsLeaderboardMetric {
    pub fn parse(query: Option<&str>) -> Result<Self, String> {
        match query_param_value(query, "metric").as_deref() {
            None | Some("requests") => Ok(Self::Requests),
            Some("tokens") => Ok(Self::Tokens),
            Some("cost") => Ok(Self::Cost),
            Some(_) => Err("metric must be one of: requests, tokens, cost".to_string()),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Requests => "requests",
            Self::Tokens => "tokens",
            Self::Cost => "cost",
        }
    }
}

impl AdminStatsSortOrder {
    pub fn parse(query: Option<&str>) -> Result<Self, String> {
        match query_param_value(query, "order").as_deref() {
            None | Some("desc") => Ok(Self::Desc),
            Some("asc") => Ok(Self::Asc),
            Some(_) => Err("order must be one of: asc, desc".to_string()),
        }
    }
}

impl AdminStatsUsageFilter {
    pub fn from_query(query: Option<&str>) -> Self {
        Self {
            user_id: query_param_value(query, "user_id"),
            provider_name: query_param_value(query, "provider_name"),
            model: query_param_value(query, "model"),
        }
    }
}

impl AdminStatsTimeSeriesBucket {
    pub fn add_usage(&mut self, item: &StoredRequestUsageAudit) {
        self.total_requests = self.total_requests.saturating_add(1);
        self.input_tokens = self.input_tokens.saturating_add(item.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(item.output_tokens);
        self.cache_creation_tokens = self
            .cache_creation_tokens
            .saturating_add(item.cache_creation_input_tokens);
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(item.cache_read_input_tokens);
        self.total_cost += item.total_cost_usd;
        self.total_response_time_ms += item.response_time_ms.unwrap_or(0) as f64;
    }

    pub fn merge(&mut self, other: &Self) {
        self.total_requests = self.total_requests.saturating_add(other.total_requests);
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.cache_creation_tokens = self
            .cache_creation_tokens
            .saturating_add(other.cache_creation_tokens);
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(other.cache_read_tokens);
        self.total_cost += other.total_cost;
        self.total_response_time_ms += other.total_response_time_ms;
    }

    pub fn avg_response_time_ms(&self) -> f64 {
        if self.total_requests == 0 {
            0.0
        } else {
            self.total_response_time_ms / self.total_requests as f64
        }
    }

    pub fn to_json_with_avg(&self, date: String) -> serde_json::Value {
        json!({
            "date": date,
            "total_requests": self.total_requests,
            "input_tokens": self.input_tokens,
            "output_tokens": self.output_tokens,
            "cache_creation_tokens": self.cache_creation_tokens,
            "cache_read_tokens": self.cache_read_tokens,
            "total_cost": round_to(self.total_cost, 6),
            "avg_response_time_ms": round_to(self.avg_response_time_ms(), 2),
        })
    }

    pub fn to_json_without_avg(&self, date: String) -> serde_json::Value {
        json!({
            "date": date,
            "total_requests": self.total_requests,
            "input_tokens": self.input_tokens,
            "output_tokens": self.output_tokens,
            "cache_creation_tokens": self.cache_creation_tokens,
            "cache_read_tokens": self.cache_read_tokens,
            "total_cost": round_to(self.total_cost, 6),
        })
    }
}

impl AdminStatsTimeRange {
    pub fn resolve_optional(query: Option<&str>) -> Result<Option<Self>, String> {
        let tz_offset_minutes = parse_tz_offset_minutes(query)?;
        let start_date = query_param_value(query, "start_date")
            .map(|value| parse_naive_date("start_date", &value))
            .transpose()?;
        let end_date = query_param_value(query, "end_date")
            .map(|value| parse_naive_date("end_date", &value))
            .transpose()?;
        let preset = query_param_value(query, "preset");

        if preset.is_none() && start_date.is_none() && end_date.is_none() {
            let default_days = admin_usage_default_days();
            if default_days == 0 {
                return Ok(None);
            }
            let end_date = user_today(tz_offset_minutes);
            let start_date = end_date
                .checked_sub_signed(chrono::Duration::days(
                    i64::try_from(default_days.saturating_sub(1)).unwrap_or(0),
                ))
                .unwrap_or(end_date);
            return Ok(Some(Self {
                start_date,
                end_date,
                tz_offset_minutes,
            }));
        }

        let (start_date, end_date) = match (preset.as_deref(), start_date, end_date) {
            (Some(preset), None, None) => resolve_preset_dates(preset, tz_offset_minutes)?,
            (None, Some(start_date), Some(end_date)) => (start_date, end_date),
            (Some(_), Some(_), _) | (Some(_), _, Some(_)) => {
                return Err("preset cannot be combined with start_date or end_date".to_string());
            }
            _ => {
                return Err(
                    "Either preset or both start_date and end_date must be provided".to_string(),
                );
            }
        };

        if start_date > end_date {
            return Err("start_date must be <= end_date".to_string());
        }

        let days = (end_date - start_date).num_days();
        if days > 365 {
            return Err("Query range cannot exceed 365 days".to_string());
        }

        Ok(Some(Self {
            start_date,
            end_date,
            tz_offset_minutes,
        }))
    }

    pub fn resolve_required(
        query: Option<&str>,
        start_key: &str,
        end_key: &str,
    ) -> Result<Self, String> {
        let tz_offset_minutes = parse_tz_offset_minutes(query)?;
        let start_date = query_param_value(query, start_key)
            .ok_or_else(|| format!("{start_key} is required"))
            .and_then(|value| parse_naive_date(start_key, &value))?;
        let end_date = query_param_value(query, end_key)
            .ok_or_else(|| format!("{end_key} is required"))
            .and_then(|value| parse_naive_date(end_key, &value))?;

        if start_date > end_date {
            return Err(format!("{start_key} must be <= {end_key}"));
        }

        Ok(Self {
            start_date,
            end_date,
            tz_offset_minutes,
        })
    }

    pub fn to_unix_bounds(&self) -> Option<(u64, u64)> {
        let offset = chrono::Duration::minutes(i64::from(self.tz_offset_minutes));
        let start_local = self.start_date.and_hms_opt(0, 0, 0)?;
        let end_local = self
            .end_date
            .checked_add_signed(chrono::Duration::days(1))?
            .and_hms_opt(0, 0, 0)?;
        let start_utc =
            chrono::DateTime::<Utc>::from_naive_utc_and_offset(start_local - offset, Utc)
                .timestamp();
        let end_utc =
            chrono::DateTime::<Utc>::from_naive_utc_and_offset(end_local - offset, Utc).timestamp();
        if start_utc < 0 || end_utc <= 0 {
            return None;
        }
        Some((start_utc as u64, end_utc as u64))
    }

    pub fn to_utc_datetime_bounds(&self) -> Option<(chrono::DateTime<Utc>, chrono::DateTime<Utc>)> {
        let offset = chrono::Duration::minutes(i64::from(self.tz_offset_minutes));
        let start_local = self.start_date.and_hms_opt(0, 0, 0)?;
        let end_local = self
            .end_date
            .checked_add_signed(chrono::Duration::days(1))?
            .and_hms_opt(0, 0, 0)?;
        Some((
            chrono::DateTime::<Utc>::from_naive_utc_and_offset(start_local - offset, Utc),
            chrono::DateTime::<Utc>::from_naive_utc_and_offset(end_local - offset, Utc),
        ))
    }

    pub fn validate_for_time_series(
        &self,
        granularity: AdminStatsGranularity,
    ) -> Result<(), String> {
        if granularity == AdminStatsGranularity::Hour && self.start_date != self.end_date {
            return Err("Hour granularity only supports single day query".to_string());
        }
        let days_inclusive = (self.end_date - self.start_date).num_days() + 1;
        if days_inclusive > 90 {
            return Err(format!(
                "Time series query range cannot exceed 90 days (requested {days_inclusive} days). For longer ranges, use aggregated statistics instead."
            ));
        }
        Ok(())
    }

    pub fn local_dates(&self) -> Vec<chrono::NaiveDate> {
        let mut current = self.start_date;
        let mut dates = Vec::new();
        while current <= self.end_date {
            dates.push(current);
            let Some(next) = current.checked_add_signed(chrono::Duration::days(1)) else {
                break;
            };
            current = next;
        }
        dates
    }

    pub fn local_date_strings(&self) -> Vec<String> {
        self.local_dates()
            .into_iter()
            .map(|date| date.to_string())
            .collect()
    }

    pub fn local_date_for_unix_secs(&self, unix_secs: u64) -> Option<chrono::NaiveDate> {
        let timestamp = chrono::DateTime::<Utc>::from_timestamp(i64::try_from(unix_secs).ok()?, 0)?;
        let local = timestamp
            .checked_add_signed(chrono::Duration::minutes(i64::from(self.tz_offset_minutes)))?;
        Some(local.date_naive())
    }

    pub fn local_date_string_for_unix_secs(&self, unix_secs: u64) -> Option<String> {
        Some(self.local_date_for_unix_secs(unix_secs)?.to_string())
    }
}

pub fn round_to(value: f64, decimals: u32) -> f64 {
    let factor = 10_f64.powi(i32::try_from(decimals).unwrap_or(0));
    (value * factor).round() / factor
}

pub fn admin_stats_provider_quota_usage_empty_response() -> Response<Body> {
    Json(json!({
        "providers": [],
        "data_source_available": false,
    }))
    .into_response()
}

pub fn admin_stats_cost_forecast_empty_response() -> Response<Body> {
    Json(json!({
        "history": [],
        "forecast": [],
        "slope": 0.0,
        "intercept": 0.0,
        "data_source_available": false,
    }))
    .into_response()
}

pub fn admin_stats_comparison_empty_response(
    current_range: &AdminStatsTimeRange,
    comparison_range: &AdminStatsTimeRange,
) -> Response<Body> {
    Json(json!({
        "current": {
            "total_requests": 0,
            "total_tokens": 0,
            "total_cost": 0.0,
            "actual_total_cost": 0.0,
            "avg_response_time_ms": 0.0,
            "error_requests": 0,
        },
        "comparison": {
            "total_requests": 0,
            "total_tokens": 0,
            "total_cost": 0.0,
            "actual_total_cost": 0.0,
            "avg_response_time_ms": 0.0,
            "error_requests": 0,
        },
        "change_percent": {
            "total_requests": serde_json::Value::Null,
            "total_tokens": serde_json::Value::Null,
            "total_cost": serde_json::Value::Null,
            "actual_total_cost": serde_json::Value::Null,
            "avg_response_time_ms": serde_json::Value::Null,
            "error_requests": serde_json::Value::Null,
        },
        "current_start": current_range.start_date.to_string(),
        "current_end": current_range.end_date.to_string(),
        "comparison_start": comparison_range.start_date.to_string(),
        "comparison_end": comparison_range.end_date.to_string(),
    }))
    .into_response()
}

pub fn admin_stats_error_distribution_empty_response() -> Response<Body> {
    Json(json!({
        "distribution": [],
        "trend": [],
    }))
    .into_response()
}

pub fn admin_stats_performance_percentiles_empty_response() -> Response<Body> {
    Json(json!([])).into_response()
}

pub fn admin_stats_cost_savings_empty_response() -> Response<Body> {
    Json(json!({
        "cache_read_tokens": 0,
        "cache_read_cost": 0.0,
        "cache_creation_cost": 0.0,
        "estimated_full_cost": 0.0,
        "cache_savings": 0.0,
    }))
    .into_response()
}

pub fn admin_stats_leaderboard_empty_response(
    metric: AdminStatsLeaderboardMetric,
    time_range: Option<&AdminStatsTimeRange>,
) -> Response<Body> {
    Json(json!({
        "items": [],
        "total": 0,
        "metric": metric.as_str(),
        "start_date": time_range.map(|value| value.start_date.to_string()),
        "end_date": time_range.map(|value| value.end_date.to_string()),
    }))
    .into_response()
}

pub fn admin_stats_time_series_empty_response() -> Response<Body> {
    Json(json!([])).into_response()
}

pub fn admin_stats_bad_request_response(detail: String) -> Response<Body> {
    (
        http::StatusCode::BAD_REQUEST,
        Json(json!({ "detail": detail })),
    )
        .into_response()
}

pub fn build_admin_stats_provider_quota_usage_response(
    providers: &[StoredProviderCatalogProvider],
    now: chrono::DateTime<Utc>,
) -> Response<Body> {
    let now_unix_secs = now.timestamp().max(0) as u64;
    let now_day = u64::from(now.day());
    let mut payload: Vec<_> = providers
        .iter()
        .filter(|provider| {
            provider.billing_type.as_deref() == Some("monthly_quota")
                || provider.monthly_quota_usd.is_some()
        })
        .map(|provider| {
            let quota = provider.monthly_quota_usd.unwrap_or(0.0);
            let used = provider.monthly_used_usd.unwrap_or(0.0);
            let remaining = (quota - used).max(0.0);
            let usage_percent = if quota > 0.0 {
                round_to((used / quota) * 100.0, 2)
            } else {
                0.0
            };

            let days_elapsed = provider
                .quota_last_reset_at_unix_secs
                .map(|reset_at| ((now_unix_secs.saturating_sub(reset_at)) / 86_400).max(1))
                .unwrap_or_else(|| now_day.saturating_sub(1).max(1));
            let daily_rate = if used > 0.0 {
                used / days_elapsed as f64
            } else {
                0.0
            };
            let estimated_exhaust_at_unix_secs = if daily_rate > 0.0 && remaining > 0.0 {
                let estimated = now_unix_secs
                    .saturating_add(((remaining / daily_rate) * 86_400.0).max(0.0) as u64);
                Some(
                    provider
                        .quota_expires_at_unix_secs
                        .map(|quota_expires_at| quota_expires_at.min(estimated))
                        .unwrap_or(estimated),
                )
            } else {
                provider.quota_expires_at_unix_secs
            };

            json!({
                "id": provider.id,
                "name": provider.name,
                "quota_usd": quota,
                "used_usd": used,
                "remaining_usd": remaining,
                "usage_percent": usage_percent,
                "quota_expires_at": provider.quota_expires_at_unix_secs.and_then(unix_secs_to_rfc3339),
                "estimated_exhaust_at": estimated_exhaust_at_unix_secs.and_then(unix_secs_to_rfc3339),
            })
        })
        .collect();

    payload.sort_by(|left, right| {
        let left_value = left
            .get("usage_percent")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        let right_value = right
            .get("usage_percent")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        right_value.total_cmp(&left_value)
    });

    Json(json!({ "providers": payload })).into_response()
}

pub fn build_admin_stats_leaderboard_response(
    metric: AdminStatsLeaderboardMetric,
    time_range: Option<&AdminStatsTimeRange>,
    leaderboard: &[AdminStatsLeaderboardItem],
    offset: usize,
    limit: usize,
    name_mode: AdminStatsLeaderboardNameMode,
) -> Response<Body> {
    let total = leaderboard.len();
    let items: Vec<_> = leaderboard
        .iter()
        .enumerate()
        .skip(offset)
        .take(limit)
        .map(|(index, item)| {
            let rank = compute_dense_rank(metric, leaderboard, index);
            let value = match metric {
                AdminStatsLeaderboardMetric::Requests => json!(item.requests),
                AdminStatsLeaderboardMetric::Tokens => json!(item.tokens),
                AdminStatsLeaderboardMetric::Cost => json!(round_to(item.cost, 6)),
            };
            let name = match name_mode {
                AdminStatsLeaderboardNameMode::Id => item.id.clone(),
                AdminStatsLeaderboardNameMode::Name => item.name.clone(),
            };
            json!({
                "rank": rank,
                "id": item.id,
                "name": name,
                "value": value,
                "requests": item.requests,
                "tokens": item.tokens,
                "cost": round_to(item.cost, 6),
            })
        })
        .collect();

    Json(json!({
        "items": items,
        "total": total,
        "metric": metric.as_str(),
        "start_date": time_range.map(|value| value.start_date.to_string()),
        "end_date": time_range.map(|value| value.end_date.to_string()),
    }))
    .into_response()
}

pub fn build_admin_stats_comparison_response(
    current_usage: &[StoredRequestUsageAudit],
    comparison_usage: &[StoredRequestUsageAudit],
    current_range: &AdminStatsTimeRange,
    comparison_range: &AdminStatsTimeRange,
) -> Response<Body> {
    let current = aggregate_usage_stats(current_usage);
    let comparison = aggregate_usage_stats(comparison_usage);
    build_admin_stats_comparison_response_from_aggregates(
        &current,
        &comparison,
        current_range,
        comparison_range,
    )
}

pub fn build_admin_stats_comparison_response_from_aggregates(
    current: &AdminStatsAggregate,
    comparison: &AdminStatsAggregate,
    current_range: &AdminStatsTimeRange,
    comparison_range: &AdminStatsTimeRange,
) -> Response<Body> {
    Json(json!({
        "current": {
            "total_requests": current.total_requests,
            "total_tokens": current.total_tokens,
            "total_cost": round_to(current.total_cost, 6),
            "actual_total_cost": round_to(current.actual_total_cost, 6),
            "avg_response_time_ms": round_to(current.avg_response_time_ms(), 2),
            "error_requests": current.error_requests,
        },
        "comparison": {
            "total_requests": comparison.total_requests,
            "total_tokens": comparison.total_tokens,
            "total_cost": round_to(comparison.total_cost, 6),
            "actual_total_cost": round_to(comparison.actual_total_cost, 6),
            "avg_response_time_ms": round_to(comparison.avg_response_time_ms(), 2),
            "error_requests": comparison.error_requests,
        },
        "change_percent": {
            "total_requests": pct_change_value(current.total_requests as f64, comparison.total_requests as f64),
            "total_tokens": pct_change_value(current.total_tokens as f64, comparison.total_tokens as f64),
            "total_cost": pct_change_value(current.total_cost, comparison.total_cost),
            "actual_total_cost": pct_change_value(current.actual_total_cost, comparison.actual_total_cost),
            "avg_response_time_ms": pct_change_value(current.avg_response_time_ms(), comparison.avg_response_time_ms()),
            "error_requests": pct_change_value(current.error_requests as f64, comparison.error_requests as f64),
        },
        "current_start": current_range.start_date.to_string(),
        "current_end": current_range.end_date.to_string(),
        "comparison_start": comparison_range.start_date.to_string(),
        "comparison_end": comparison_range.end_date.to_string(),
    }))
    .into_response()
}

pub fn build_admin_stats_error_distribution_response(
    time_range: &AdminStatsTimeRange,
    usage: &[StoredRequestUsageAudit],
) -> Response<Body> {
    let mut distribution: std::collections::BTreeMap<String, u64> =
        std::collections::BTreeMap::new();
    let mut trend: std::collections::BTreeMap<String, std::collections::BTreeMap<String, u64>> =
        std::collections::BTreeMap::new();

    for item in usage {
        let Some(category) = item
            .error_category
            .as_ref()
            .filter(|value| !value.trim().is_empty())
        else {
            continue;
        };
        let Some(local_day) = time_range.local_date_string_for_unix_secs(item.created_at_unix_ms)
        else {
            continue;
        };
        *distribution.entry(category.clone()).or_default() += 1;
        *trend
            .entry(local_day)
            .or_default()
            .entry(category.clone())
            .or_default() += 1;
    }

    let mut distribution_items: Vec<_> = distribution
        .into_iter()
        .map(|(category, count)| json!({ "category": category, "count": count }))
        .collect();
    distribution_items.sort_by(|left, right| {
        let left_count = left
            .get("count")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let right_count = right
            .get("count")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        right_count.cmp(&left_count).then_with(|| {
            left.get("category")
                .and_then(serde_json::Value::as_str)
                .cmp(&right.get("category").and_then(serde_json::Value::as_str))
        })
    });

    let trend_items: Vec<_> = trend
        .into_iter()
        .map(|(date, categories)| {
            let total: u64 = categories.values().copied().sum();
            json!({
                "date": date,
                "total": total,
                "categories": categories,
            })
        })
        .collect();

    Json(json!({
        "distribution": distribution_items,
        "trend": trend_items,
    }))
    .into_response()
}

pub fn build_admin_stats_performance_percentiles_response(
    time_range: &AdminStatsTimeRange,
    usage: &[StoredRequestUsageAudit],
) -> Response<Body> {
    let mut by_day: std::collections::BTreeMap<String, (Vec<u64>, Vec<u64>)> = time_range
        .local_date_strings()
        .into_iter()
        .map(|date| (date, (Vec::new(), Vec::new())))
        .collect();

    for item in usage {
        if item.status != "completed" {
            continue;
        }
        let Some(local_day) = time_range.local_date_string_for_unix_secs(item.created_at_unix_ms)
        else {
            continue;
        };
        let Some((response_times, first_byte_times)) = by_day.get_mut(&local_day) else {
            continue;
        };
        if let Some(response_time_ms) = item.response_time_ms {
            response_times.push(response_time_ms);
        }
        if let Some(first_byte_time_ms) = item.first_byte_time_ms {
            first_byte_times.push(first_byte_time_ms);
        }
    }

    let payload: Vec<_> = by_day
        .into_iter()
        .map(|(date, (mut response_times, mut first_byte_times))| {
            json!({
                "date": date,
                "p50_response_time_ms": percentile_cont(&mut response_times, 0.5),
                "p90_response_time_ms": percentile_cont(&mut response_times, 0.9),
                "p99_response_time_ms": percentile_cont(&mut response_times, 0.99),
                "p50_first_byte_time_ms": percentile_cont(&mut first_byte_times, 0.5),
                "p90_first_byte_time_ms": percentile_cont(&mut first_byte_times, 0.9),
                "p99_first_byte_time_ms": percentile_cont(&mut first_byte_times, 0.99),
            })
        })
        .collect();

    Json(serde_json::Value::Array(payload)).into_response()
}

pub fn build_admin_stats_time_series_response(
    time_range: &AdminStatsTimeRange,
    granularity: AdminStatsGranularity,
    usage: &[StoredRequestUsageAudit],
) -> Response<Body> {
    Json(serde_json::Value::Array(build_time_series_payload(
        time_range,
        granularity,
        usage,
    )))
    .into_response()
}

fn admin_stats_time_series_bucket_from_summary(
    bucket: &StoredUsageTimeSeriesBucket,
) -> AdminStatsTimeSeriesBucket {
    AdminStatsTimeSeriesBucket {
        total_requests: bucket.total_requests,
        input_tokens: bucket.input_tokens,
        output_tokens: bucket.output_tokens,
        cache_creation_tokens: bucket.cache_creation_tokens,
        cache_read_tokens: bucket.cache_read_tokens,
        total_cost: bucket.total_cost_usd,
        total_response_time_ms: bucket.total_response_time_ms,
    }
}

fn build_daily_time_series_buckets_from_summaries(
    time_range: &AdminStatsTimeRange,
    buckets: &[StoredUsageTimeSeriesBucket],
) -> std::collections::BTreeMap<chrono::NaiveDate, AdminStatsTimeSeriesBucket> {
    let mut values: std::collections::BTreeMap<chrono::NaiveDate, AdminStatsTimeSeriesBucket> =
        time_range
            .local_dates()
            .into_iter()
            .map(|date| (date, AdminStatsTimeSeriesBucket::default()))
            .collect();

    for bucket in buckets {
        let Ok(date) = chrono::NaiveDate::parse_from_str(&bucket.bucket_key, "%Y-%m-%d") else {
            continue;
        };
        let Some(value) = values.get_mut(&date) else {
            continue;
        };
        value.merge(&admin_stats_time_series_bucket_from_summary(bucket));
    }

    values
}

fn build_daily_time_series_payload_from_summaries(
    time_range: &AdminStatsTimeRange,
    buckets: &[StoredUsageTimeSeriesBucket],
) -> Vec<serde_json::Value> {
    build_daily_time_series_buckets_from_summaries(time_range, buckets)
        .into_iter()
        .map(|(date, bucket)| bucket.to_json_with_avg(date.to_string()))
        .collect()
}

fn build_weekly_time_series_payload_from_summaries(
    time_range: &AdminStatsTimeRange,
    buckets: &[StoredUsageTimeSeriesBucket],
) -> Vec<serde_json::Value> {
    let mut weekly: std::collections::BTreeMap<
        (i32, u32),
        (chrono::NaiveDate, AdminStatsTimeSeriesBucket),
    > = std::collections::BTreeMap::new();

    for (date, bucket) in build_daily_time_series_buckets_from_summaries(time_range, buckets) {
        let iso = date.iso_week();
        let entry = weekly
            .entry((iso.year(), iso.week()))
            .or_insert_with(|| (date, AdminStatsTimeSeriesBucket::default()));
        entry.0 = entry.0.min(date);
        entry.1.merge(&bucket);
    }

    weekly
        .into_values()
        .map(|(date, bucket)| bucket.to_json_with_avg(date.to_string()))
        .collect()
}

fn build_monthly_time_series_payload_from_summaries(
    time_range: &AdminStatsTimeRange,
    buckets: &[StoredUsageTimeSeriesBucket],
) -> Vec<serde_json::Value> {
    let mut monthly: std::collections::BTreeMap<
        (i32, u32),
        (chrono::NaiveDate, AdminStatsTimeSeriesBucket),
    > = std::collections::BTreeMap::new();

    for (date, bucket) in build_daily_time_series_buckets_from_summaries(time_range, buckets) {
        let Some(month_start) = chrono::NaiveDate::from_ymd_opt(date.year(), date.month(), 1)
        else {
            continue;
        };
        let entry = monthly
            .entry((date.year(), date.month()))
            .or_insert_with(|| (month_start, AdminStatsTimeSeriesBucket::default()));
        entry.1.merge(&bucket);
    }

    monthly
        .into_values()
        .map(|(date, bucket)| bucket.to_json_with_avg(date.to_string()))
        .collect()
}

fn build_hourly_time_series_payload_from_summaries(
    time_range: &AdminStatsTimeRange,
    buckets: &[StoredUsageTimeSeriesBucket],
) -> Vec<serde_json::Value> {
    let Some((mut current, end)) = time_range.to_utc_datetime_bounds() else {
        return Vec::new();
    };
    let offset = chrono::Duration::minutes(i64::from(time_range.tz_offset_minutes));
    let mut values: std::collections::BTreeMap<String, AdminStatsTimeSeriesBucket> =
        std::collections::BTreeMap::new();

    while current < end {
        let label = (current + offset)
            .format("%Y-%m-%dT%H:00:00+00:00")
            .to_string();
        values.insert(label, AdminStatsTimeSeriesBucket::default());
        let Some(next) = current.checked_add_signed(chrono::Duration::hours(1)) else {
            break;
        };
        current = next;
    }

    for bucket in buckets {
        let Some(value) = values.get_mut(&bucket.bucket_key) else {
            continue;
        };
        value.merge(&admin_stats_time_series_bucket_from_summary(bucket));
    }

    values
        .into_iter()
        .map(|(date, bucket)| bucket.to_json_without_avg(date))
        .collect()
}

pub fn build_time_series_payload_from_summaries(
    time_range: &AdminStatsTimeRange,
    granularity: AdminStatsGranularity,
    buckets: &[StoredUsageTimeSeriesBucket],
) -> Vec<serde_json::Value> {
    match granularity {
        AdminStatsGranularity::Hour => {
            build_hourly_time_series_payload_from_summaries(time_range, buckets)
        }
        AdminStatsGranularity::Day => {
            build_daily_time_series_payload_from_summaries(time_range, buckets)
        }
        AdminStatsGranularity::Week => {
            build_weekly_time_series_payload_from_summaries(time_range, buckets)
        }
        AdminStatsGranularity::Month => {
            build_monthly_time_series_payload_from_summaries(time_range, buckets)
        }
    }
}

pub fn build_admin_stats_time_series_response_from_summaries(
    time_range: &AdminStatsTimeRange,
    granularity: AdminStatsGranularity,
    buckets: &[StoredUsageTimeSeriesBucket],
) -> Response<Body> {
    Json(serde_json::Value::Array(
        build_time_series_payload_from_summaries(time_range, granularity, buckets),
    ))
    .into_response()
}

pub fn build_admin_stats_cost_forecast_response(
    time_range: &AdminStatsTimeRange,
    forecast_days: u32,
    usage: &[StoredRequestUsageAudit],
) -> Response<Body> {
    let daily = build_daily_time_series_buckets(time_range, usage);
    let history: Vec<AdminStatsForecastPoint> = daily
        .into_iter()
        .map(|(date, bucket)| AdminStatsForecastPoint {
            date,
            total_cost: bucket.total_cost,
        })
        .collect();
    let values: Vec<f64> = history.iter().map(|item| item.total_cost).collect();
    let (slope, intercept) = linear_regression(&values);
    let last_date = history
        .last()
        .map(|item| item.date)
        .unwrap_or(time_range.end_date);
    let forecast: Vec<_> = (0..forecast_days)
        .map(|index| {
            let idx = values.len() + index as usize;
            let predicted = (slope * idx as f64 + intercept).max(0.0);
            json!({
                "date": last_date
                    .checked_add_signed(chrono::Duration::days(i64::from(index + 1)))
                    .unwrap_or(last_date)
                    .to_string(),
                "total_cost": round_to(predicted, 4),
            })
        })
        .collect();

    Json(json!({
        "history": history.into_iter().map(|item| json!({
            "date": item.date.to_string(),
            "total_cost": round_to(item.total_cost, 6),
        })).collect::<Vec<_>>(),
        "forecast": forecast,
        "slope": round_to(slope, 6),
        "intercept": round_to(intercept, 6),
        "start_date": time_range.start_date.to_string(),
        "end_date": time_range.end_date.to_string(),
    }))
    .into_response()
}

pub fn build_admin_stats_cost_savings_response(
    usage: &[StoredRequestUsageAudit],
) -> Response<Body> {
    let cache_read_tokens: u64 = usage.iter().map(|item| item.cache_read_input_tokens).sum();
    let cache_read_cost: f64 = usage.iter().map(|item| item.cache_read_cost_usd).sum();
    let cache_creation_cost: f64 = usage.iter().map(|item| item.cache_creation_cost_usd).sum();
    let mut estimated_full_cost: f64 = usage
        .iter()
        .map(|item| {
            item.settlement_output_price_per_1m().unwrap_or(0.0)
                * item.cache_read_input_tokens as f64
                / 1_000_000.0
        })
        .sum();
    if estimated_full_cost <= 0.0 && cache_read_cost > 0.0 {
        estimated_full_cost = cache_read_cost * 10.0;
    }
    let cache_savings = estimated_full_cost - cache_read_cost;

    Json(json!({
        "cache_read_tokens": cache_read_tokens,
        "cache_read_cost": round_to(cache_read_cost, 6),
        "cache_creation_cost": round_to(cache_creation_cost, 6),
        "estimated_full_cost": round_to(estimated_full_cost, 6),
        "cache_savings": round_to(cache_savings, 6),
    }))
    .into_response()
}

fn unix_secs_to_rfc3339(unix_secs: u64) -> Option<String> {
    let timestamp = chrono::DateTime::<Utc>::from_timestamp(i64::try_from(unix_secs).ok()?, 0)?;
    Some(timestamp.to_rfc3339())
}

pub fn build_time_series_payload(
    time_range: &AdminStatsTimeRange,
    granularity: AdminStatsGranularity,
    items: &[StoredRequestUsageAudit],
) -> Vec<serde_json::Value> {
    match granularity {
        AdminStatsGranularity::Hour => build_hourly_time_series_payload(time_range, items),
        AdminStatsGranularity::Day => build_daily_time_series_payload(time_range, items),
        AdminStatsGranularity::Week => build_weekly_time_series_payload(time_range, items),
        AdminStatsGranularity::Month => build_monthly_time_series_payload(time_range, items),
    }
}

pub fn build_daily_time_series_buckets(
    time_range: &AdminStatsTimeRange,
    items: &[StoredRequestUsageAudit],
) -> std::collections::BTreeMap<chrono::NaiveDate, AdminStatsTimeSeriesBucket> {
    let mut buckets: std::collections::BTreeMap<chrono::NaiveDate, AdminStatsTimeSeriesBucket> =
        time_range
            .local_dates()
            .into_iter()
            .map(|date| (date, AdminStatsTimeSeriesBucket::default()))
            .collect();

    for item in items {
        let Some(local_day) = time_range.local_date_for_unix_secs(item.created_at_unix_ms) else {
            continue;
        };
        let Some(bucket) = buckets.get_mut(&local_day) else {
            continue;
        };
        bucket.add_usage(item);
    }

    buckets
}

fn build_daily_time_series_payload(
    time_range: &AdminStatsTimeRange,
    items: &[StoredRequestUsageAudit],
) -> Vec<serde_json::Value> {
    build_daily_time_series_buckets(time_range, items)
        .into_iter()
        .map(|(date, bucket)| bucket.to_json_with_avg(date.to_string()))
        .collect()
}

fn build_weekly_time_series_payload(
    time_range: &AdminStatsTimeRange,
    items: &[StoredRequestUsageAudit],
) -> Vec<serde_json::Value> {
    let mut weekly: std::collections::BTreeMap<
        (i32, u32),
        (chrono::NaiveDate, AdminStatsTimeSeriesBucket),
    > = std::collections::BTreeMap::new();

    for (date, bucket) in build_daily_time_series_buckets(time_range, items) {
        let iso = date.iso_week();
        let entry = weekly
            .entry((iso.year(), iso.week()))
            .or_insert_with(|| (date, AdminStatsTimeSeriesBucket::default()));
        entry.0 = entry.0.min(date);
        entry.1.merge(&bucket);
    }

    weekly
        .into_values()
        .map(|(date, bucket)| bucket.to_json_with_avg(date.to_string()))
        .collect()
}

fn build_monthly_time_series_payload(
    time_range: &AdminStatsTimeRange,
    items: &[StoredRequestUsageAudit],
) -> Vec<serde_json::Value> {
    let mut monthly: std::collections::BTreeMap<
        (i32, u32),
        (chrono::NaiveDate, AdminStatsTimeSeriesBucket),
    > = std::collections::BTreeMap::new();

    for (date, bucket) in build_daily_time_series_buckets(time_range, items) {
        let Some(month_start) = chrono::NaiveDate::from_ymd_opt(date.year(), date.month(), 1)
        else {
            continue;
        };
        let entry = monthly
            .entry((date.year(), date.month()))
            .or_insert_with(|| (month_start, AdminStatsTimeSeriesBucket::default()));
        entry.1.merge(&bucket);
    }

    monthly
        .into_values()
        .map(|(date, bucket)| bucket.to_json_with_avg(date.to_string()))
        .collect()
}

fn build_hourly_time_series_payload(
    time_range: &AdminStatsTimeRange,
    items: &[StoredRequestUsageAudit],
) -> Vec<serde_json::Value> {
    let Some((mut current, end)) = time_range.to_utc_datetime_bounds() else {
        return Vec::new();
    };
    let offset = chrono::Duration::minutes(i64::from(time_range.tz_offset_minutes));
    let mut buckets: std::collections::BTreeMap<String, AdminStatsTimeSeriesBucket> =
        std::collections::BTreeMap::new();

    while current < end {
        let label = (current + offset)
            .format("%Y-%m-%dT%H:00:00+00:00")
            .to_string();
        buckets.insert(label, AdminStatsTimeSeriesBucket::default());
        let Some(next) = current.checked_add_signed(chrono::Duration::hours(1)) else {
            break;
        };
        current = next;
    }

    for item in items {
        let Some(unix_secs) = i64::try_from(item.created_at_unix_ms).ok() else {
            continue;
        };
        let Some(timestamp) = chrono::DateTime::<Utc>::from_timestamp(unix_secs, 0) else {
            continue;
        };
        let Some(local) = timestamp.checked_add_signed(offset) else {
            continue;
        };
        let label = local.format("%Y-%m-%dT%H:00:00+00:00").to_string();
        let Some(bucket) = buckets.get_mut(&label) else {
            continue;
        };
        bucket.add_usage(item);
    }

    buckets
        .into_iter()
        .map(|(date, bucket)| bucket.to_json_without_avg(date))
        .collect()
}

pub fn aggregate_usage_stats(items: &[StoredRequestUsageAudit]) -> AdminStatsAggregate {
    let mut aggregate = AdminStatsAggregate::default();
    for item in items {
        aggregate.total_requests = aggregate.total_requests.saturating_add(1);
        aggregate.total_tokens = aggregate.total_tokens.saturating_add(item.total_tokens);
        aggregate.total_cost += item.total_cost_usd;
        aggregate.actual_total_cost += item.actual_total_cost_usd;
        aggregate.total_response_time_ms += item.response_time_ms.unwrap_or(0) as f64;
        if item.status_code.is_some_and(|value| value >= 400) || item.error_message.is_some() {
            aggregate.error_requests = aggregate.error_requests.saturating_add(1);
        }
    }
    aggregate
}

pub fn percentile_cont(values: &mut [u64], percentile: f64) -> Option<u64> {
    if values.len() < MIN_PERCENTILE_SAMPLES {
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

pub fn pct_change_value(current: f64, previous: f64) -> serde_json::Value {
    if previous == 0.0 {
        if current == 0.0 {
            json!(0.0)
        } else {
            serde_json::Value::Null
        }
    } else {
        json!(round_to((current - previous) / previous * 100.0, 2))
    }
}

pub fn linear_regression(values: &[f64]) -> (f64, f64) {
    let n = values.len();
    if n <= 1 {
        return (0.0, values.first().copied().unwrap_or(0.0));
    }
    let sum_x: f64 = (0..n).map(|value| value as f64).sum();
    let sum_y: f64 = values.iter().sum();
    let sum_x2: f64 = (0..n).map(|value| (value * value) as f64).sum();
    let sum_xy: f64 = values
        .iter()
        .enumerate()
        .map(|(index, value)| index as f64 * *value)
        .sum();
    let n = n as f64;
    let denom = n * sum_x2 - sum_x * sum_x;
    if denom == 0.0 {
        return (0.0, values.last().copied().unwrap_or(0.0));
    }
    let slope = (n * sum_xy - sum_x * sum_y) / denom;
    let intercept = (sum_y - slope * sum_x) / n;
    (slope, intercept)
}

pub fn build_model_leaderboard_items(
    items: &[StoredRequestUsageAudit],
) -> Vec<AdminStatsLeaderboardItem> {
    let mut grouped: std::collections::BTreeMap<String, AdminStatsLeaderboardItem> =
        std::collections::BTreeMap::new();
    for item in items {
        if matches!(item.status.as_str(), "pending" | "streaming")
            || matches!(item.provider_name.as_str(), "unknown" | "pending")
        {
            continue;
        }
        let entry =
            grouped
                .entry(item.model.clone())
                .or_insert_with(|| AdminStatsLeaderboardItem {
                    id: item.model.clone(),
                    name: item.model.clone(),
                    requests: 0,
                    tokens: 0,
                    cost: 0.0,
                });
        entry.requests = entry.requests.saturating_add(1);
        entry.tokens = entry.tokens.saturating_add(
            item.input_tokens
                .saturating_add(item.output_tokens)
                .saturating_add(item.cache_creation_input_tokens)
                .saturating_add(item.cache_read_input_tokens),
        );
        entry.cost += item.total_cost_usd;
    }
    grouped.into_values().collect()
}

pub fn build_model_leaderboard_items_from_summaries(
    items: &[StoredUsageLeaderboardSummary],
) -> Vec<AdminStatsLeaderboardItem> {
    items
        .iter()
        .map(|item| AdminStatsLeaderboardItem {
            id: item.group_key.clone(),
            name: item.group_key.clone(),
            requests: item.request_count,
            tokens: item.total_tokens,
            cost: item.total_cost_usd,
        })
        .collect()
}

pub fn build_user_leaderboard_items(
    items: &[StoredRequestUsageAudit],
    users: &std::collections::BTreeMap<String, AdminStatsUserMetadata>,
    auth_user_reader_available: bool,
    include_inactive: bool,
    exclude_admin: bool,
) -> Vec<AdminStatsLeaderboardItem> {
    let mut grouped: std::collections::BTreeMap<String, AdminStatsLeaderboardItem> =
        std::collections::BTreeMap::new();

    for item in items {
        if matches!(item.status.as_str(), "pending" | "streaming")
            || matches!(item.provider_name.as_str(), "unknown" | "pending")
        {
            continue;
        }
        let Some(user_id) = item.user_id.as_deref() else {
            continue;
        };
        let entry_name = if let Some(user) = users.get(user_id) {
            if user.is_deleted {
                continue;
            }
            if !include_inactive && !user.is_active {
                continue;
            }
            if exclude_admin && user.role.eq_ignore_ascii_case("admin") {
                continue;
            }
            user.name.clone()
        } else {
            if exclude_admin {
                continue;
            }
            if auth_user_reader_available {
                user_id.to_string()
            } else {
                item.username
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| user_id.to_string())
            }
        };

        let entry =
            grouped
                .entry(user_id.to_string())
                .or_insert_with(|| AdminStatsLeaderboardItem {
                    id: user_id.to_string(),
                    name: entry_name,
                    requests: 0,
                    tokens: 0,
                    cost: 0.0,
                });
        entry.requests = entry.requests.saturating_add(1);
        entry.tokens = entry.tokens.saturating_add(
            item.input_tokens
                .saturating_add(item.output_tokens)
                .saturating_add(item.cache_creation_input_tokens)
                .saturating_add(item.cache_read_input_tokens),
        );
        entry.cost += item.total_cost_usd;
    }

    grouped.into_values().collect()
}

pub fn build_user_leaderboard_items_from_summaries(
    items: &[StoredUsageLeaderboardSummary],
    users: &std::collections::BTreeMap<String, AdminStatsUserMetadata>,
    auth_user_reader_available: bool,
    user_reader_available: bool,
    include_inactive: bool,
    exclude_admin: bool,
) -> Vec<AdminStatsLeaderboardItem> {
    let mut grouped = Vec::new();

    for item in items {
        let user_id = item.group_key.as_str();
        let entry_name = if let Some(user) = users.get(user_id) {
            if user.is_deleted {
                continue;
            }
            if !include_inactive && !user.is_active {
                continue;
            }
            if exclude_admin && user.role.eq_ignore_ascii_case("admin") {
                continue;
            }
            user.name.clone()
        } else {
            if exclude_admin {
                continue;
            }
            if auth_user_reader_available || user_reader_available {
                user_id.to_string()
            } else {
                item.legacy_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| user_id.to_string())
            }
        };

        grouped.push(AdminStatsLeaderboardItem {
            id: user_id.to_string(),
            name: entry_name,
            requests: item.request_count,
            tokens: item.total_tokens,
            cost: item.total_cost_usd,
        });
    }

    grouped
}

pub fn build_api_key_leaderboard_items(
    items: &[StoredRequestUsageAudit],
    snapshots: Option<&[StoredAuthApiKeySnapshot]>,
    api_key_names: &std::collections::BTreeMap<String, String>,
    include_inactive: bool,
    exclude_admin: bool,
) -> Vec<AdminStatsLeaderboardItem> {
    let snapshot_by_api_key_id: std::collections::BTreeMap<_, _> = snapshots
        .unwrap_or(&[])
        .iter()
        .map(|snapshot| (snapshot.api_key_id.as_str(), snapshot))
        .collect();
    let mut grouped: std::collections::BTreeMap<String, AdminStatsLeaderboardItem> =
        std::collections::BTreeMap::new();
    let snapshots_available = snapshots.is_some();

    for item in items {
        if matches!(item.status.as_str(), "pending" | "streaming")
            || matches!(item.provider_name.as_str(), "unknown" | "pending")
        {
            continue;
        }
        let Some(api_key_id) = item.api_key_id.as_deref() else {
            continue;
        };

        let entry_name = if let Some(snapshot) = snapshot_by_api_key_id.get(api_key_id) {
            if snapshot.user_is_deleted {
                continue;
            }
            if !include_inactive && !snapshot.api_key_is_active {
                continue;
            }
            if exclude_admin && snapshot.user_role.eq_ignore_ascii_case("admin") {
                continue;
            }
            api_key_names
                .get(api_key_id)
                .cloned()
                .unwrap_or_else(|| api_key_id.to_string())
        } else {
            if snapshots_available {
                continue;
            }
            api_key_names
                .get(api_key_id)
                .cloned()
                .or_else(|| item.api_key_name.clone())
                .unwrap_or_else(|| api_key_id.to_string())
        };

        let entry =
            grouped
                .entry(api_key_id.to_string())
                .or_insert_with(|| AdminStatsLeaderboardItem {
                    id: api_key_id.to_string(),
                    name: entry_name,
                    requests: 0,
                    tokens: 0,
                    cost: 0.0,
                });
        entry.requests = entry.requests.saturating_add(1);
        entry.tokens = entry.tokens.saturating_add(
            item.input_tokens
                .saturating_add(item.output_tokens)
                .saturating_add(item.cache_creation_input_tokens)
                .saturating_add(item.cache_read_input_tokens),
        );
        entry.cost += item.total_cost_usd;
    }

    grouped.into_values().collect()
}

pub fn build_api_key_leaderboard_items_from_summaries(
    items: &[StoredUsageLeaderboardSummary],
    snapshots: Option<&[StoredAuthApiKeySnapshot]>,
    api_key_names: &std::collections::BTreeMap<String, String>,
    include_inactive: bool,
    exclude_admin: bool,
) -> Vec<AdminStatsLeaderboardItem> {
    let snapshot_by_api_key_id: std::collections::BTreeMap<_, _> = snapshots
        .unwrap_or(&[])
        .iter()
        .map(|snapshot| (snapshot.api_key_id.as_str(), snapshot))
        .collect();
    let snapshots_available = snapshots.is_some();
    let mut grouped = Vec::new();

    for item in items {
        let api_key_id = item.group_key.as_str();
        let entry_name = if let Some(snapshot) = snapshot_by_api_key_id.get(api_key_id) {
            if snapshot.user_is_deleted {
                continue;
            }
            if !include_inactive && !snapshot.api_key_is_active {
                continue;
            }
            if exclude_admin && snapshot.user_role.eq_ignore_ascii_case("admin") {
                continue;
            }
            api_key_names
                .get(api_key_id)
                .cloned()
                .unwrap_or_else(|| api_key_id.to_string())
        } else {
            if snapshots_available {
                continue;
            }
            api_key_names
                .get(api_key_id)
                .cloned()
                .or_else(|| item.legacy_name.clone())
                .unwrap_or_else(|| api_key_id.to_string())
        };

        grouped.push(AdminStatsLeaderboardItem {
            id: api_key_id.to_string(),
            name: entry_name,
            requests: item.request_count,
            tokens: item.total_tokens,
            cost: item.total_cost_usd,
        });
    }

    grouped
}

pub fn compare_leaderboard_items(
    metric: AdminStatsLeaderboardMetric,
    order: AdminStatsSortOrder,
    left: &AdminStatsLeaderboardItem,
    right: &AdminStatsLeaderboardItem,
) -> std::cmp::Ordering {
    let metric_order = match metric {
        AdminStatsLeaderboardMetric::Requests => left.requests.cmp(&right.requests),
        AdminStatsLeaderboardMetric::Tokens => left.tokens.cmp(&right.tokens),
        AdminStatsLeaderboardMetric::Cost => left
            .cost
            .partial_cmp(&right.cost)
            .unwrap_or(std::cmp::Ordering::Equal),
    };
    let metric_order = match order {
        AdminStatsSortOrder::Asc => metric_order,
        AdminStatsSortOrder::Desc => metric_order.reverse(),
    };
    if metric_order == std::cmp::Ordering::Equal {
        left.id.cmp(&right.id)
    } else {
        metric_order
    }
}

fn leaderboard_metric_equal(
    metric: AdminStatsLeaderboardMetric,
    left: &AdminStatsLeaderboardItem,
    right: &AdminStatsLeaderboardItem,
) -> bool {
    match metric {
        AdminStatsLeaderboardMetric::Requests => left.requests == right.requests,
        AdminStatsLeaderboardMetric::Tokens => left.tokens == right.tokens,
        AdminStatsLeaderboardMetric::Cost => (left.cost - right.cost).abs() < 1e-9,
    }
}

pub fn compute_dense_rank(
    metric: AdminStatsLeaderboardMetric,
    items: &[AdminStatsLeaderboardItem],
    index: usize,
) -> usize {
    if index == 0 {
        return 1;
    }
    let mut rank = 1usize;
    for current in 1..=index {
        if !leaderboard_metric_equal(metric, &items[current - 1], &items[current]) {
            rank = rank.saturating_add(1);
        }
    }
    rank
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        build_api_key_leaderboard_items, build_user_leaderboard_items, AdminStatsUserMetadata,
    };
    use aether_data::repository::auth::StoredAuthApiKeySnapshot;
    use aether_data_contracts::repository::usage::StoredRequestUsageAudit;

    fn sample_usage(api_key_name: Option<&str>) -> StoredRequestUsageAudit {
        StoredRequestUsageAudit::new(
            "usage-1".to_string(),
            "req-1".to_string(),
            Some("user-1".to_string()),
            Some("key-1".to_string()),
            Some("alice".to_string()),
            api_key_name.map(str::to_string),
            "OpenAI".to_string(),
            "gpt-5".to_string(),
            None,
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
            false,
            false,
            10,
            20,
            30,
            0.3,
            0.3,
            Some(200),
            None,
            None,
            Some(400),
            Some(120),
            "completed".to_string(),
            "settled".to_string(),
            1_711_000_000,
            1_711_000_001,
            Some(1_711_000_002),
        )
        .expect("usage should build")
    }

    fn sample_api_key_snapshot(api_key_name: Option<&str>) -> StoredAuthApiKeySnapshot {
        StoredAuthApiKeySnapshot {
            user_id: "user-1".to_string(),
            username: "alice".to_string(),
            email: Some("alice@example.com".to_string()),
            user_role: "user".to_string(),
            user_auth_source: "local".to_string(),
            user_is_active: true,
            user_is_deleted: false,
            user_rate_limit: None,
            user_allowed_providers: None,
            user_allowed_api_formats: None,
            user_allowed_models: None,
            api_key_id: "key-1".to_string(),
            api_key_name: api_key_name.map(str::to_string),
            api_key_is_active: true,
            api_key_is_locked: false,
            api_key_is_standalone: false,
            api_key_rate_limit: None,
            api_key_concurrent_limit: None,
            api_key_expires_at_unix_secs: None,
            api_key_allowed_providers: None,
            api_key_allowed_api_formats: None,
            api_key_allowed_models: None,
        }
    }

    #[test]
    fn api_key_leaderboard_prefers_resolved_names_over_legacy_usage_names_when_snapshots_exist() {
        let leaderboard = build_api_key_leaderboard_items(
            &[sample_usage(Some("legacy-default"))],
            Some(&[sample_api_key_snapshot(None)]),
            &BTreeMap::from([("key-1".to_string(), "fresh-default".to_string())]),
            false,
            false,
        );

        assert_eq!(leaderboard.len(), 1);
        assert_eq!(leaderboard[0].id, "key-1");
        assert_eq!(leaderboard[0].name, "fresh-default");
    }

    #[test]
    fn api_key_leaderboard_keeps_legacy_usage_name_fallback_without_snapshot_reader() {
        let leaderboard = build_api_key_leaderboard_items(
            &[sample_usage(Some("legacy-default"))],
            None,
            &BTreeMap::new(),
            false,
            false,
        );

        assert_eq!(leaderboard.len(), 1);
        assert_eq!(leaderboard[0].name, "legacy-default");
    }

    #[test]
    fn user_leaderboard_prefers_resolved_names_over_legacy_usage_names_when_reader_exists() {
        let leaderboard = build_user_leaderboard_items(
            &[sample_usage(Some("legacy-default"))],
            &BTreeMap::from([(
                "user-1".to_string(),
                AdminStatsUserMetadata {
                    name: "fresh-alice".to_string(),
                    role: "user".to_string(),
                    is_active: true,
                    is_deleted: false,
                },
            )]),
            true,
            false,
            false,
        );

        assert_eq!(leaderboard.len(), 1);
        assert_eq!(leaderboard[0].id, "user-1");
        assert_eq!(leaderboard[0].name, "fresh-alice");
    }

    #[test]
    fn user_leaderboard_does_not_fallback_to_legacy_usage_name_when_reader_exists() {
        let leaderboard = build_user_leaderboard_items(
            &[sample_usage(Some("legacy-default"))],
            &BTreeMap::new(),
            true,
            false,
            false,
        );

        assert_eq!(leaderboard.len(), 1);
        assert_eq!(leaderboard[0].name, "user-1");
    }

    #[test]
    fn user_leaderboard_keeps_legacy_usage_name_fallback_without_reader() {
        let leaderboard = build_user_leaderboard_items(
            &[sample_usage(Some("legacy-default"))],
            &BTreeMap::new(),
            false,
            false,
            false,
        );

        assert_eq!(leaderboard.len(), 1);
        assert_eq!(leaderboard[0].name, "alice");
    }
}
