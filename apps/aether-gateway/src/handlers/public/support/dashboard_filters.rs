use super::{
    build_auth_error_response, query_param_value, resolve_authenticated_local_user, AppState,
    GatewayError, GatewayPublicRequestContext,
};
use aether_data_contracts::repository::usage::{
    StoredUsageCostSavingsSummary, StoredUsageDashboardDailyBreakdownRow,
    StoredUsageDashboardSummary, UsageAuditAggregationGroupBy, UsageAuditAggregationQuery,
    UsageCostSavingsSummaryQuery, UsageDashboardDailyBreakdownQuery,
    UsageDashboardProviderCountsQuery, UsageDashboardSummaryQuery,
};
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use chrono::Datelike;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

use crate::query::dashboard_stats::{
    list_admin_dashboard_daily_model_aggregates, list_admin_dashboard_daily_provider_aggregates,
    list_admin_dashboard_daily_totals_aggregates, list_admin_dashboard_hourly_model_aggregates,
    list_admin_dashboard_hourly_provider_aggregates, list_admin_dashboard_hourly_totals_aggregates,
    list_user_dashboard_daily_model_aggregates, list_user_dashboard_daily_totals_aggregates,
    list_user_dashboard_hourly_model_aggregates, list_user_dashboard_hourly_totals_aggregates,
    read_stats_hourly_cutoff, summarize_dashboard_usage_from_daily_aggregates,
    DashboardDailyModelAggregateRow, DashboardDailyProviderAggregateRow,
    DashboardDailyTotalsAggregateRow,
};
use crate::query::usage_heatmap::read_stats_daily_cutoff_date;

#[derive(Debug, Clone, Copy)]
struct DashboardDateRange {
    start_date: chrono::NaiveDate,
    end_date: chrono::NaiveDate,
    tz_offset_minutes: i32,
}

#[derive(Debug, Default, Clone)]
struct DashboardUsageTotals {
    requests: u64,
    input_tokens: u64,
    effective_input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    cache_hit_total_input_context: u64,
    cache_creation_cost_usd: f64,
    cache_read_cost_usd: f64,
    total_cost_usd: f64,
    actual_total_cost_usd: f64,
    error_requests: u64,
    response_time_sum_ms: f64,
    response_time_samples: u64,
}

#[derive(Debug, Default, Clone)]
struct DashboardModelAggregate {
    requests: u64,
    tokens: u64,
    cost: f64,
    response_time_sum_ms: f64,
    response_time_samples: u64,
}

#[derive(Debug, Default, Clone)]
struct DashboardProviderAggregate {
    requests: u64,
    tokens: u64,
    cost: f64,
}

#[derive(Debug, Default, Clone)]
struct DashboardDailyAggregate {
    totals: DashboardUsageTotals,
    models: std::collections::BTreeMap<String, DashboardModelAggregate>,
    providers: std::collections::BTreeMap<String, DashboardProviderAggregate>,
}

pub(super) fn decision_route_kind(request_context: &GatewayPublicRequestContext) -> Option<&str> {
    request_context
        .control_decision
        .as_ref()
        .and_then(|decision| decision.route_kind.as_deref())
}

impl DashboardUsageTotals {
    fn absorb_summary(&mut self, summary: &StoredUsageDashboardSummary) {
        self.requests = self.requests.saturating_add(summary.total_requests);
        self.input_tokens = self.input_tokens.saturating_add(summary.input_tokens);
        self.effective_input_tokens = self
            .effective_input_tokens
            .saturating_add(summary.effective_input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(summary.output_tokens);
        self.total_tokens = self.total_tokens.saturating_add(summary.total_tokens);
        self.cache_creation_tokens = self
            .cache_creation_tokens
            .saturating_add(summary.cache_creation_tokens);
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(summary.cache_read_tokens);
        self.cache_hit_total_input_context = self
            .cache_hit_total_input_context
            .saturating_add(summary.total_input_context);
        self.cache_creation_cost_usd += summary.cache_creation_cost_usd;
        self.cache_read_cost_usd += summary.cache_read_cost_usd;
        self.total_cost_usd += summary.total_cost_usd;
        self.actual_total_cost_usd += summary.actual_total_cost_usd;
        self.error_requests = self.error_requests.saturating_add(summary.error_requests);
        self.response_time_sum_ms += summary.response_time_sum_ms;
        self.response_time_samples = self
            .response_time_samples
            .saturating_add(summary.response_time_samples);
    }

    fn avg_response_time_seconds(&self) -> f64 {
        if self.response_time_samples == 0 {
            0.0
        } else {
            dashboard_round_f64(
                (self.response_time_sum_ms / self.response_time_samples as f64) / 1000.0,
                4,
            )
        }
    }

    fn cache_hit_rate(&self) -> f64 {
        if self.cache_hit_total_input_context == 0 {
            0.0
        } else {
            dashboard_round_f64(
                self.cache_read_tokens as f64 / self.cache_hit_total_input_context as f64 * 100.0,
                2,
            )
        }
    }
}

fn dashboard_round_f64(value: f64, decimals: u32) -> f64 {
    let factor = 10_f64.powi(i32::try_from(decimals).unwrap_or_default());
    (value * factor).round() / factor
}

fn dashboard_format_integer(value: u64) -> String {
    let digits = value.to_string();
    let mut formatted = String::new();
    for (index, ch) in digits.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            formatted.push(',');
        }
        formatted.push(ch);
    }
    formatted.chars().rev().collect()
}

fn dashboard_trimmed_decimal(value: f64, decimals: usize) -> String {
    let mut formatted = format!("{value:.decimals$}");
    while formatted.contains('.') && formatted.ends_with('0') {
        formatted.pop();
    }
    if formatted.ends_with('.') {
        formatted.pop();
    }
    formatted
}

fn dashboard_format_token_compact(value: u64) -> String {
    if value < 1_000 {
        return dashboard_format_integer(value);
    }

    if value < 1_000_000 {
        let thousands = value as f64 / 1_000.0;
        if thousands >= 100.0 {
            return format!("{}K", thousands.round() as u64);
        }
        let decimals = if thousands >= 10.0 { 1 } else { 2 };
        return format!("{}K", dashboard_trimmed_decimal(thousands, decimals));
    }

    let millions = value as f64 / 1_000_000.0;
    if millions >= 100.0 {
        return format!("{}M", millions.round() as u64);
    }
    let decimals = if millions >= 10.0 { 1 } else { 2 };
    format!("{}M", dashboard_trimmed_decimal(millions, decimals))
}

fn dashboard_format_usd(value: f64) -> String {
    format!("${:.2}", dashboard_round_f64(value, 2))
}

fn dashboard_format_percentage(value: f64) -> String {
    format!("{:.1}%", dashboard_round_f64(value, 1))
}

fn dashboard_format_token_subvalue(totals: &DashboardUsageTotals) -> String {
    format!(
        "输入 {} / 输出 {}",
        dashboard_format_integer(totals.input_tokens),
        dashboard_format_integer(totals.output_tokens)
    )
}

fn dashboard_format_today_token_subvalue(totals: &DashboardUsageTotals) -> String {
    format!(
        "输入 {} / 输出 {} · 写缓存 {} / 读缓存 {}",
        dashboard_format_token_compact(totals.effective_input_tokens),
        dashboard_format_token_compact(totals.output_tokens),
        dashboard_format_token_compact(totals.cache_creation_tokens),
        dashboard_format_token_compact(totals.cache_read_tokens)
    )
}

fn dashboard_today_token_value_from_subvalue(totals: &DashboardUsageTotals) -> u64 {
    totals
        .effective_input_tokens
        .saturating_add(totals.output_tokens)
        .saturating_add(totals.cache_creation_tokens)
        .saturating_add(totals.cache_read_tokens)
}

fn dashboard_parse_tz_offset_minutes(query: Option<&str>) -> Result<i32, String> {
    query_param_value(query, "tz_offset_minutes")
        .map(|value| {
            value
                .parse::<i32>()
                .map_err(|_| "tz_offset_minutes must be a valid integer".to_string())
        })
        .transpose()
        .map(|value| value.unwrap_or(0))
}

fn dashboard_parse_naive_date(field: &str, value: &str) -> Result<chrono::NaiveDate, String> {
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| format!("{field} must be a valid date in YYYY-MM-DD format"))
}

fn dashboard_parse_days(
    query: Option<&str>,
    default: u32,
    min: u32,
    max: u32,
) -> Result<u32, String> {
    match query_param_value(query, "days") {
        Some(value) => {
            let parsed = value
                .parse::<u32>()
                .map_err(|_| format!("days must be between {min} and {max}"))?;
            if (min..=max).contains(&parsed) {
                Ok(parsed)
            } else {
                Err(format!("days must be between {min} and {max}"))
            }
        }
        None => Ok(default),
    }
}

fn dashboard_user_today(tz_offset_minutes: i32) -> chrono::NaiveDate {
    (chrono::Utc::now() + chrono::Duration::minutes(i64::from(tz_offset_minutes))).date_naive()
}

fn dashboard_resolve_preset_dates(
    preset: &str,
    tz_offset_minutes: i32,
) -> Result<(chrono::NaiveDate, chrono::NaiveDate), String> {
    let user_today = dashboard_user_today(tz_offset_minutes);
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
        "this_month" => Ok((user_today.with_day(1).unwrap_or(user_today), user_today)),
        "last_month" => {
            let first_of_this_month = user_today.with_day(1).unwrap_or(user_today);
            let last_month_end = first_of_this_month
                .checked_sub_signed(chrono::Duration::days(1))
                .unwrap_or(first_of_this_month);
            Ok((
                last_month_end.with_day(1).unwrap_or(last_month_end),
                last_month_end,
            ))
        }
        "this_year" => Ok((
            chrono::NaiveDate::from_ymd_opt(user_today.year(), 1, 1).unwrap_or(user_today),
            user_today,
        )),
        _ => Err("Invalid preset".to_string()),
    }
}

fn dashboard_build_range_from_days(days: u32, tz_offset_minutes: i32) -> DashboardDateRange {
    let end_date = dashboard_user_today(tz_offset_minutes);
    let start_date = end_date
        .checked_sub_signed(chrono::Duration::days(i64::from(days.saturating_sub(1))))
        .unwrap_or(end_date);
    DashboardDateRange {
        start_date,
        end_date,
        tz_offset_minutes,
    }
}

fn dashboard_parse_stats_range(query: Option<&str>) -> Result<DashboardDateRange, String> {
    let tz_offset_minutes = dashboard_parse_tz_offset_minutes(query)?;
    let preset = query_param_value(query, "preset");
    let start_date = query_param_value(query, "start_date")
        .map(|value| dashboard_parse_naive_date("start_date", &value))
        .transpose()?;
    let end_date = query_param_value(query, "end_date")
        .map(|value| dashboard_parse_naive_date("end_date", &value))
        .transpose()?;

    let (start_date, end_date) = if let Some(preset) = preset.as_deref() {
        if start_date.is_some() || end_date.is_some() {
            return Err("preset cannot be combined with start_date or end_date".to_string());
        }
        dashboard_resolve_preset_dates(preset, tz_offset_minutes)?
    } else if let (Some(start_date), Some(end_date)) = (start_date, end_date) {
        (start_date, end_date)
    } else if start_date.is_some() || end_date.is_some() {
        return Err("start_date and end_date must be provided together".to_string());
    } else if query_param_value(query, "days").is_some() {
        let range = dashboard_build_range_from_days(
            dashboard_parse_days(query, 30, 1, 365)?,
            tz_offset_minutes,
        );
        return Ok(range);
    } else {
        dashboard_resolve_preset_dates("this_month", tz_offset_minutes)?
    };

    if start_date > end_date {
        return Err("start_date must be <= end_date".to_string());
    }

    Ok(DashboardDateRange {
        start_date,
        end_date,
        tz_offset_minutes,
    })
}

fn dashboard_parse_daily_range(query: Option<&str>) -> Result<DashboardDateRange, String> {
    let tz_offset_minutes = dashboard_parse_tz_offset_minutes(query)?;
    let preset = query_param_value(query, "preset");
    let start_date = query_param_value(query, "start_date")
        .map(|value| dashboard_parse_naive_date("start_date", &value))
        .transpose()?;
    let end_date = query_param_value(query, "end_date")
        .map(|value| dashboard_parse_naive_date("end_date", &value))
        .transpose()?;

    let range = if let Some(preset) = preset.as_deref() {
        if start_date.is_some() || end_date.is_some() {
            return Err("preset cannot be combined with start_date or end_date".to_string());
        }
        let (start_date, end_date) = dashboard_resolve_preset_dates(preset, tz_offset_minutes)?;
        DashboardDateRange {
            start_date,
            end_date,
            tz_offset_minutes,
        }
    } else if let (Some(start_date), Some(end_date)) = (start_date, end_date) {
        DashboardDateRange {
            start_date,
            end_date,
            tz_offset_minutes,
        }
    } else if start_date.is_some() || end_date.is_some() {
        return Err("start_date and end_date must be provided together".to_string());
    } else {
        dashboard_build_range_from_days(dashboard_parse_days(query, 7, 1, 30)?, tz_offset_minutes)
    };

    if range.start_date > range.end_date {
        return Err("start_date must be <= end_date".to_string());
    }

    Ok(range)
}

fn dashboard_range_bounds_unix(range: DashboardDateRange) -> Option<(u64, u64)> {
    let offset = chrono::Duration::minutes(i64::from(range.tz_offset_minutes));
    let start_local = range.start_date.and_hms_opt(0, 0, 0)?;
    let end_local = range
        .end_date
        .checked_add_signed(chrono::Duration::days(1))?
        .and_hms_opt(0, 0, 0)?;
    let start_utc = (start_local - offset).and_utc().timestamp();
    let end_utc = (end_local - offset).and_utc().timestamp();
    Some((start_utc.max(0) as u64, end_utc.max(0) as u64))
}

fn dashboard_range_bounds_utc(
    range: DashboardDateRange,
) -> Option<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)> {
    let (created_from_unix_secs, created_until_unix_secs) = dashboard_range_bounds_unix(range)?;
    let start_utc =
        chrono::DateTime::<chrono::Utc>::from_timestamp(created_from_unix_secs as i64, 0)?;
    let end_utc =
        chrono::DateTime::<chrono::Utc>::from_timestamp(created_until_unix_secs as i64, 0)?;
    Some((start_utc, end_utc))
}

fn dashboard_local_day_bounds_utc_exclusive(
    range: DashboardDateRange,
) -> Option<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)> {
    let offset = chrono::Duration::minutes(i64::from(range.tz_offset_minutes));
    let start_local = range.start_date.and_hms_opt(0, 0, 0)?;
    let end_exclusive_local = range
        .end_date
        .checked_add_signed(chrono::Duration::days(1))?
        .and_hms_opt(0, 0, 0)?;
    let start_utc = chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(
        start_local.checked_sub_signed(offset)?,
        chrono::Utc,
    );
    let end_exclusive_utc = chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(
        end_exclusive_local.checked_sub_signed(offset)?,
        chrono::Utc,
    );
    Some((start_utc, end_exclusive_utc))
}

fn dashboard_utc_midnight(value: chrono::DateTime<chrono::Utc>) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(
        value
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .expect("midnight should be valid"),
        chrono::Utc,
    )
}

fn dashboard_next_utc_midnight(
    value: chrono::DateTime<chrono::Utc>,
) -> chrono::DateTime<chrono::Utc> {
    let midnight = dashboard_utc_midnight(value);
    if value == midnight {
        midnight
    } else {
        midnight + chrono::Duration::days(1)
    }
}

fn dashboard_unix_secs(value: chrono::DateTime<chrono::Utc>) -> u64 {
    value.timestamp().max(0) as u64
}

fn dashboard_absorb_dashboard_summary(
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

async fn dashboard_summary_for_unix_range_raw(
    state: &AppState,
    created_from_unix_secs: u64,
    created_until_unix_secs: u64,
    user_id: Option<&str>,
    error_context: &str,
) -> Result<StoredUsageDashboardSummary, Response<Body>> {
    match state
        .summarize_dashboard_usage(&UsageDashboardSummaryQuery {
            created_from_unix_secs,
            created_until_unix_secs,
            user_id: user_id.map(ToOwned::to_owned),
        })
        .await
    {
        Ok(value) => Ok(value),
        Err(err) => Err(build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("{error_context}: {err:?}"),
            false,
        )),
    }
}

async fn dashboard_daily_breakdown_for_unix_range_raw(
    state: &AppState,
    created_from_unix_secs: u64,
    created_until_unix_secs: u64,
    tz_offset_minutes: i32,
    user_id: Option<&str>,
    error_context: &str,
) -> Result<Vec<StoredUsageDashboardDailyBreakdownRow>, Response<Body>> {
    match state
        .list_dashboard_daily_breakdown(&UsageDashboardDailyBreakdownQuery {
            created_from_unix_secs,
            created_until_unix_secs,
            tz_offset_minutes,
            user_id: user_id.map(ToOwned::to_owned),
        })
        .await
    {
        Ok(value) => Ok(value),
        Err(err) => Err(build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("{error_context}: {err:?}"),
            false,
        )),
    }
}

async fn dashboard_summary_for_range_raw(
    state: &AppState,
    range: DashboardDateRange,
    user_id: Option<&str>,
    error_context: &str,
) -> Result<StoredUsageDashboardSummary, Response<Body>> {
    let Some((created_from_unix_secs, created_until_unix_secs)) =
        dashboard_range_bounds_unix(range)
    else {
        return Err(build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("{error_context}: invalid time range"),
            false,
        ));
    };

    dashboard_summary_for_unix_range_raw(
        state,
        created_from_unix_secs,
        created_until_unix_secs,
        user_id,
        error_context,
    )
    .await
}

async fn dashboard_summary_for_range(
    state: &AppState,
    range: DashboardDateRange,
    user_id: Option<&str>,
    error_context: &str,
) -> Result<StoredUsageDashboardSummary, Response<Body>> {
    let Some(pool) = state.postgres_pool() else {
        return dashboard_summary_for_range_raw(state, range, user_id, error_context).await;
    };
    let cutoff_date = match read_stats_daily_cutoff_date(&pool).await {
        Ok(value) => value,
        Err(err) => {
            return Err(build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("{error_context}: {err:?}"),
                false,
            ))
        }
    };
    let Some(cutoff_date) = cutoff_date else {
        return dashboard_summary_for_range_raw(state, range, user_id, error_context).await;
    };
    let Some((start_utc, end_utc)) = dashboard_range_bounds_utc(range) else {
        return Err(build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("{error_context}: invalid time range"),
            false,
        ));
    };

    let aggregate_start = dashboard_next_utc_midnight(start_utc);
    let aggregate_end = dashboard_utc_midnight(end_utc).min(cutoff_date);
    let mut summary = StoredUsageDashboardSummary::default();

    if aggregate_start < aggregate_end {
        let leading_end = aggregate_start.min(end_utc);
        if start_utc < leading_end {
            let raw = dashboard_summary_for_unix_range_raw(
                state,
                dashboard_unix_secs(start_utc),
                dashboard_unix_secs(leading_end),
                user_id,
                error_context,
            )
            .await?;
            dashboard_absorb_dashboard_summary(&mut summary, &raw);
        }

        let aggregate = summarize_dashboard_usage_from_daily_aggregates(
            &pool,
            aggregate_start,
            aggregate_end,
            user_id,
        )
        .await
        .map_err(|err| {
            build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("{error_context}: {err:?}"),
                false,
            )
        })?;
        dashboard_absorb_dashboard_summary(&mut summary, &aggregate);
        if aggregate_end < end_utc {
            let raw = dashboard_summary_for_unix_range_raw(
                state,
                dashboard_unix_secs(aggregate_end),
                dashboard_unix_secs(end_utc),
                user_id,
                error_context,
            )
            .await?;
            dashboard_absorb_dashboard_summary(&mut summary, &raw);
        }
    } else if start_utc < end_utc {
        let raw = dashboard_summary_for_unix_range_raw(
            state,
            dashboard_unix_secs(start_utc),
            dashboard_unix_secs(end_utc),
            user_id,
            error_context,
        )
        .await?;
        dashboard_absorb_dashboard_summary(&mut summary, &raw);
    }

    Ok(summary)
}

async fn dashboard_daily_breakdown_for_range(
    state: &AppState,
    range: DashboardDateRange,
    user_id: Option<&str>,
    error_context: &str,
) -> Result<Vec<StoredUsageDashboardDailyBreakdownRow>, Response<Body>> {
    let Some((created_from_unix_secs, created_until_unix_secs)) =
        dashboard_range_bounds_unix(range)
    else {
        return Err(build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("{error_context}: invalid time range"),
            false,
        ));
    };

    match state
        .list_dashboard_daily_breakdown(&UsageDashboardDailyBreakdownQuery {
            created_from_unix_secs,
            created_until_unix_secs,
            tz_offset_minutes: range.tz_offset_minutes,
            user_id: user_id.map(ToOwned::to_owned),
        })
        .await
    {
        Ok(value) => Ok(value),
        Err(err) => Err(build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("{error_context}: {err:?}"),
            false,
        )),
    }
}

fn dashboard_apply_daily_breakdown_rows(
    usage: &[StoredUsageDashboardDailyBreakdownRow],
    by_date: &mut std::collections::BTreeMap<chrono::NaiveDate, DashboardDailyAggregate>,
    model_summary: &mut std::collections::BTreeMap<String, DashboardModelAggregate>,
    provider_summary: &mut std::collections::BTreeMap<String, DashboardProviderAggregate>,
) {
    for row in usage {
        let Ok(date) = chrono::NaiveDate::parse_from_str(&row.date, "%Y-%m-%d") else {
            continue;
        };
        let aggregate = by_date.entry(date).or_default();
        dashboard_daily_aggregate_record(aggregate, row);

        let model = model_summary.entry(row.model.clone()).or_default();
        model.requests = model.requests.saturating_add(row.requests);
        model.tokens = model.tokens.saturating_add(row.total_tokens);
        model.cost += row.total_cost_usd;
        model.response_time_sum_ms += row.response_time_sum_ms;
        model.response_time_samples = model
            .response_time_samples
            .saturating_add(row.response_time_samples);

        let provider = provider_summary.entry(row.provider.clone()).or_default();
        provider.requests = provider.requests.saturating_add(row.requests);
        provider.tokens = provider.tokens.saturating_add(row.total_tokens);
        provider.cost += row.total_cost_usd;
    }
}

fn dashboard_record_daily_totals_aggregate(
    by_date: &mut std::collections::BTreeMap<chrono::NaiveDate, DashboardDailyAggregate>,
    row: &DashboardDailyTotalsAggregateRow,
) {
    let Ok(date) = chrono::NaiveDate::parse_from_str(&row.date, "%Y-%m-%d") else {
        return;
    };
    let aggregate = by_date.entry(date).or_default();
    aggregate.totals.requests = aggregate.totals.requests.saturating_add(row.requests);
    aggregate.totals.total_tokens = aggregate
        .totals
        .total_tokens
        .saturating_add(row.total_tokens);
    aggregate.totals.total_cost_usd += row.total_cost_usd;
    aggregate.totals.response_time_sum_ms += row.response_time_sum_ms;
    aggregate.totals.response_time_samples = aggregate
        .totals
        .response_time_samples
        .saturating_add(row.response_time_samples);
}

fn dashboard_record_daily_model_aggregate(
    by_date: &mut std::collections::BTreeMap<chrono::NaiveDate, DashboardDailyAggregate>,
    model_summary: &mut std::collections::BTreeMap<String, DashboardModelAggregate>,
    row: &DashboardDailyModelAggregateRow,
) {
    let Ok(date) = chrono::NaiveDate::parse_from_str(&row.date, "%Y-%m-%d") else {
        return;
    };
    let aggregate = by_date.entry(date).or_default();
    let model = aggregate.models.entry(row.model.clone()).or_default();
    model.requests = model.requests.saturating_add(row.requests);
    model.tokens = model.tokens.saturating_add(row.total_tokens);
    model.cost += row.total_cost_usd;
    model.response_time_sum_ms += row.response_time_sum_ms;
    model.response_time_samples = model
        .response_time_samples
        .saturating_add(row.response_time_samples);

    let summary = model_summary.entry(row.model.clone()).or_default();
    summary.requests = summary.requests.saturating_add(row.requests);
    summary.tokens = summary.tokens.saturating_add(row.total_tokens);
    summary.cost += row.total_cost_usd;
    summary.response_time_sum_ms += row.response_time_sum_ms;
    summary.response_time_samples = summary
        .response_time_samples
        .saturating_add(row.response_time_samples);
}

fn dashboard_record_daily_provider_aggregate(
    by_date: &mut std::collections::BTreeMap<chrono::NaiveDate, DashboardDailyAggregate>,
    provider_summary: &mut std::collections::BTreeMap<String, DashboardProviderAggregate>,
    row: &DashboardDailyProviderAggregateRow,
) {
    let Ok(date) = chrono::NaiveDate::parse_from_str(&row.date, "%Y-%m-%d") else {
        return;
    };
    let aggregate = by_date.entry(date).or_default();
    let provider = aggregate.providers.entry(row.provider.clone()).or_default();
    provider.requests = provider.requests.saturating_add(row.requests);
    provider.tokens = provider.tokens.saturating_add(row.total_tokens);
    provider.cost += row.total_cost_usd;

    let summary = provider_summary.entry(row.provider.clone()).or_default();
    summary.requests = summary.requests.saturating_add(row.requests);
    summary.tokens = summary.tokens.saturating_add(row.total_tokens);
    summary.cost += row.total_cost_usd;
}

fn dashboard_build_daily_stats_payload(
    range: DashboardDateRange,
    is_admin: bool,
    by_date: &std::collections::BTreeMap<chrono::NaiveDate, DashboardDailyAggregate>,
    model_summary: &std::collections::BTreeMap<String, DashboardModelAggregate>,
    provider_summary: &std::collections::BTreeMap<String, DashboardProviderAggregate>,
) -> serde_json::Value {
    let mut daily_stats = Vec::new();
    let mut cursor = range.start_date;
    while cursor <= range.end_date {
        let payload = if let Some(aggregate) = by_date.get(&cursor) {
            let mut model_breakdown = aggregate
                .models
                .iter()
                .map(|(model, value)| {
                    json!({
                        "model": model,
                        "requests": value.requests,
                        "tokens": value.tokens,
                        "cost": dashboard_round_f64(value.cost, 4),
                    })
                })
                .collect::<Vec<_>>();
            model_breakdown.sort_by(|left, right| {
                right["cost"]
                    .as_f64()
                    .unwrap_or_default()
                    .partial_cmp(&left["cost"].as_f64().unwrap_or_default())
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| {
                        left["model"]
                            .as_str()
                            .unwrap_or_default()
                            .cmp(right["model"].as_str().unwrap_or_default())
                    })
            });

            let mut item = json!({
                "date": cursor.to_string(),
                "requests": aggregate.totals.requests,
                "tokens": aggregate.totals.total_tokens,
                "cost": dashboard_round_f64(aggregate.totals.total_cost_usd, 4),
                "avg_response_time": aggregate.totals.avg_response_time_seconds(),
                "unique_models": aggregate.models.len(),
                "model_breakdown": model_breakdown,
            });
            if is_admin {
                item["unique_providers"] = json!(aggregate.providers.len());
            }
            item
        } else {
            let mut item = json!({
                "date": cursor.to_string(),
                "requests": 0,
                "tokens": 0,
                "cost": 0.0,
                "avg_response_time": 0.0,
                "unique_models": 0,
                "model_breakdown": [],
            });
            if is_admin {
                item["unique_providers"] = json!(0);
            }
            item
        };
        daily_stats.push(payload);
        cursor = cursor
            .checked_add_signed(chrono::Duration::days(1))
            .unwrap_or(cursor + chrono::Duration::days(1));
    }

    let mut model_summary_payload = model_summary
        .iter()
        .map(|(model, value)| {
            let avg_response_time = if value.response_time_samples == 0 {
                0.0
            } else {
                dashboard_round_f64(
                    (value.response_time_sum_ms / value.response_time_samples as f64) / 1000.0,
                    4,
                )
            };
            let cost_per_request = if value.requests == 0 {
                0.0
            } else {
                dashboard_round_f64(value.cost / value.requests as f64, 4)
            };
            let tokens_per_request = if value.requests == 0 {
                0.0
            } else {
                dashboard_round_f64(value.tokens as f64 / value.requests as f64, 4)
            };
            json!({
                "model": model,
                "requests": value.requests,
                "tokens": value.tokens,
                "cost": dashboard_round_f64(value.cost, 4),
                "avg_response_time": avg_response_time,
                "cost_per_request": cost_per_request,
                "tokens_per_request": tokens_per_request,
            })
        })
        .collect::<Vec<_>>();
    model_summary_payload.sort_by(|left, right| {
        right["cost"]
            .as_f64()
            .unwrap_or_default()
            .partial_cmp(&left["cost"].as_f64().unwrap_or_default())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let provider_summary_payload = if is_admin {
        let mut items = provider_summary
            .iter()
            .map(|(provider, value)| {
                json!({
                    "provider": provider,
                    "requests": value.requests,
                    "tokens": value.tokens,
                    "cost": dashboard_round_f64(value.cost, 4),
                })
            })
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            right["cost"]
                .as_f64()
                .unwrap_or_default()
                .partial_cmp(&left["cost"].as_f64().unwrap_or_default())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Some(items)
    } else {
        None
    };

    let mut payload = json!({
        "daily_stats": daily_stats,
        "model_summary": model_summary_payload,
        "period": {
            "start_date": range.start_date.to_string(),
            "end_date": range.end_date.to_string(),
            "days": (range.end_date - range.start_date).num_days() + 1,
        },
    });
    if let Some(provider_summary_payload) = provider_summary_payload {
        payload["provider_summary"] = json!(provider_summary_payload);
    }
    payload
}

fn dashboard_range_supports_hourly_rollup(range: DashboardDateRange) -> bool {
    range.tz_offset_minutes % 60 == 0
}

async fn dashboard_admin_hourly_stats_aggregate_payload(
    state: &AppState,
    range: DashboardDateRange,
    error_context: &str,
) -> Result<Option<serde_json::Value>, Response<Body>> {
    if !dashboard_range_supports_hourly_rollup(range) {
        return Ok(None);
    }
    let Some(pool) = state.postgres_pool() else {
        return Ok(None);
    };
    let cutoff_utc = match read_stats_hourly_cutoff(&pool).await {
        Ok(value) => value,
        Err(err) => {
            return Err(build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("{error_context}: {err:?}"),
                false,
            ))
        }
    };
    let Some(cutoff_utc) = cutoff_utc else {
        return Ok(None);
    };
    let Some((range_start_utc, range_end_exclusive_utc)) =
        dashboard_local_day_bounds_utc_exclusive(range)
    else {
        return Ok(None);
    };
    let aggregate_end_utc = range_end_exclusive_utc.min(cutoff_utc);
    if range_start_utc >= aggregate_end_utc {
        return Ok(None);
    }

    let daily_totals = list_admin_dashboard_hourly_totals_aggregates(
        &pool,
        range_start_utc,
        aggregate_end_utc,
        range.tz_offset_minutes,
    )
    .await
    .map_err(|err| {
        build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("{error_context}: {err:?}"),
            false,
        )
    })?;
    let daily_models = list_admin_dashboard_hourly_model_aggregates(
        &pool,
        range_start_utc,
        aggregate_end_utc,
        range.tz_offset_minutes,
    )
    .await
    .map_err(|err| {
        build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("{error_context}: {err:?}"),
            false,
        )
    })?;
    let daily_providers = list_admin_dashboard_hourly_provider_aggregates(
        &pool,
        range_start_utc,
        aggregate_end_utc,
        range.tz_offset_minutes,
    )
    .await
    .map_err(|err| {
        build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("{error_context}: {err:?}"),
            false,
        )
    })?;

    let mut by_date =
        std::collections::BTreeMap::<chrono::NaiveDate, DashboardDailyAggregate>::new();
    let mut model_summary = std::collections::BTreeMap::<String, DashboardModelAggregate>::new();
    let mut provider_summary =
        std::collections::BTreeMap::<String, DashboardProviderAggregate>::new();

    for row in &daily_totals {
        dashboard_record_daily_totals_aggregate(&mut by_date, row);
    }
    for row in &daily_models {
        dashboard_record_daily_model_aggregate(&mut by_date, &mut model_summary, row);
    }
    for row in &daily_providers {
        dashboard_record_daily_provider_aggregate(&mut by_date, &mut provider_summary, row);
    }

    if aggregate_end_utc < range_end_exclusive_utc {
        let raw_rows = dashboard_daily_breakdown_for_unix_range_raw(
            state,
            dashboard_unix_secs(aggregate_end_utc),
            dashboard_unix_secs(range_end_exclusive_utc),
            range.tz_offset_minutes,
            None,
            error_context,
        )
        .await?;
        dashboard_apply_daily_breakdown_rows(
            &raw_rows,
            &mut by_date,
            &mut model_summary,
            &mut provider_summary,
        );
    }

    Ok(Some(dashboard_build_daily_stats_payload(
        range,
        true,
        &by_date,
        &model_summary,
        &provider_summary,
    )))
}

async fn dashboard_user_hourly_stats_aggregate_payload(
    state: &AppState,
    range: DashboardDateRange,
    user_id: &str,
    error_context: &str,
) -> Result<Option<serde_json::Value>, Response<Body>> {
    if !dashboard_range_supports_hourly_rollup(range) {
        return Ok(None);
    }
    let Some(pool) = state.postgres_pool() else {
        return Ok(None);
    };
    let cutoff_utc = match read_stats_hourly_cutoff(&pool).await {
        Ok(value) => value,
        Err(err) => {
            return Err(build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("{error_context}: {err:?}"),
                false,
            ))
        }
    };
    let Some(cutoff_utc) = cutoff_utc else {
        return Ok(None);
    };
    let Some((range_start_utc, range_end_exclusive_utc)) =
        dashboard_local_day_bounds_utc_exclusive(range)
    else {
        return Ok(None);
    };
    let aggregate_end_utc = range_end_exclusive_utc.min(cutoff_utc);
    if range_start_utc >= aggregate_end_utc {
        return Ok(None);
    }

    let daily_totals = list_user_dashboard_hourly_totals_aggregates(
        &pool,
        range_start_utc,
        aggregate_end_utc,
        range.tz_offset_minutes,
        user_id,
    )
    .await
    .map_err(|err| {
        build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("{error_context}: {err:?}"),
            false,
        )
    })?;
    let daily_models = list_user_dashboard_hourly_model_aggregates(
        &pool,
        range_start_utc,
        aggregate_end_utc,
        range.tz_offset_minutes,
        user_id,
    )
    .await
    .map_err(|err| {
        build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("{error_context}: {err:?}"),
            false,
        )
    })?;

    let mut by_date =
        std::collections::BTreeMap::<chrono::NaiveDate, DashboardDailyAggregate>::new();
    let mut model_summary = std::collections::BTreeMap::<String, DashboardModelAggregate>::new();
    let mut provider_summary =
        std::collections::BTreeMap::<String, DashboardProviderAggregate>::new();

    for row in &daily_totals {
        dashboard_record_daily_totals_aggregate(&mut by_date, row);
    }
    for row in &daily_models {
        dashboard_record_daily_model_aggregate(&mut by_date, &mut model_summary, row);
    }

    if aggregate_end_utc < range_end_exclusive_utc {
        let raw_rows = dashboard_daily_breakdown_for_unix_range_raw(
            state,
            dashboard_unix_secs(aggregate_end_utc),
            dashboard_unix_secs(range_end_exclusive_utc),
            range.tz_offset_minutes,
            Some(user_id),
            error_context,
        )
        .await?;
        dashboard_apply_daily_breakdown_rows(
            &raw_rows,
            &mut by_date,
            &mut model_summary,
            &mut provider_summary,
        );
    }

    Ok(Some(dashboard_build_daily_stats_payload(
        range,
        false,
        &by_date,
        &model_summary,
        &provider_summary,
    )))
}

async fn dashboard_admin_daily_stats_aggregate_payload(
    state: &AppState,
    range: DashboardDateRange,
    error_context: &str,
) -> Result<Option<serde_json::Value>, Response<Body>> {
    if range.tz_offset_minutes != 0 {
        return dashboard_admin_hourly_stats_aggregate_payload(state, range, error_context).await;
    }
    let Some(pool) = state.postgres_pool() else {
        return Ok(None);
    };
    let cutoff_date = match read_stats_daily_cutoff_date(&pool).await {
        Ok(value) => value,
        Err(err) => {
            return Err(build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("{error_context}: {err:?}"),
                false,
            ))
        }
    };
    let Some(cutoff_date) = cutoff_date else {
        return Ok(None);
    };

    let Some(range_end_exclusive) = range.end_date.checked_add_signed(chrono::Duration::days(1))
    else {
        return Ok(None);
    };
    let aggregate_end_exclusive = range_end_exclusive.min(cutoff_date.date_naive());
    if range.start_date >= aggregate_end_exclusive {
        return Ok(None);
    }

    let aggregate_start_utc = chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(
        range
            .start_date
            .and_hms_opt(0, 0, 0)
            .expect("midnight should be valid"),
        chrono::Utc,
    );
    let aggregate_end_utc = chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(
        aggregate_end_exclusive
            .and_hms_opt(0, 0, 0)
            .expect("midnight should be valid"),
        chrono::Utc,
    );

    let daily_totals =
        list_admin_dashboard_daily_totals_aggregates(&pool, aggregate_start_utc, aggregate_end_utc)
            .await
            .map_err(|err| {
                build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("{error_context}: {err:?}"),
                    false,
                )
            })?;
    let daily_models =
        list_admin_dashboard_daily_model_aggregates(&pool, aggregate_start_utc, aggregate_end_utc)
            .await
            .map_err(|err| {
                build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("{error_context}: {err:?}"),
                    false,
                )
            })?;
    let daily_providers = list_admin_dashboard_daily_provider_aggregates(
        &pool,
        aggregate_start_utc,
        aggregate_end_utc,
    )
    .await
    .map_err(|err| {
        build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("{error_context}: {err:?}"),
            false,
        )
    })?;

    let mut by_date =
        std::collections::BTreeMap::<chrono::NaiveDate, DashboardDailyAggregate>::new();
    let mut model_summary = std::collections::BTreeMap::<String, DashboardModelAggregate>::new();
    let mut provider_summary =
        std::collections::BTreeMap::<String, DashboardProviderAggregate>::new();

    for row in &daily_totals {
        dashboard_record_daily_totals_aggregate(&mut by_date, row);
    }
    for row in &daily_models {
        dashboard_record_daily_model_aggregate(&mut by_date, &mut model_summary, row);
    }
    for row in &daily_providers {
        dashboard_record_daily_provider_aggregate(&mut by_date, &mut provider_summary, row);
    }

    if aggregate_end_exclusive <= range.end_date {
        let raw_range = DashboardDateRange {
            start_date: aggregate_end_exclusive,
            end_date: range.end_date,
            tz_offset_minutes: range.tz_offset_minutes,
        };
        let raw_rows =
            dashboard_daily_breakdown_for_range(state, raw_range, None, error_context).await?;
        dashboard_apply_daily_breakdown_rows(
            &raw_rows,
            &mut by_date,
            &mut model_summary,
            &mut provider_summary,
        );
    }

    Ok(Some(dashboard_build_daily_stats_payload(
        range,
        true,
        &by_date,
        &model_summary,
        &provider_summary,
    )))
}

async fn dashboard_user_daily_stats_aggregate_payload(
    state: &AppState,
    range: DashboardDateRange,
    user_id: &str,
    error_context: &str,
) -> Result<Option<serde_json::Value>, Response<Body>> {
    if range.tz_offset_minutes != 0 {
        return dashboard_user_hourly_stats_aggregate_payload(state, range, user_id, error_context)
            .await;
    }
    let Some(pool) = state.postgres_pool() else {
        return Ok(None);
    };
    let cutoff_date = match read_stats_daily_cutoff_date(&pool).await {
        Ok(value) => value,
        Err(err) => {
            return Err(build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("{error_context}: {err:?}"),
                false,
            ))
        }
    };
    let Some(cutoff_date) = cutoff_date else {
        return Ok(None);
    };

    let Some(range_end_exclusive) = range.end_date.checked_add_signed(chrono::Duration::days(1))
    else {
        return Ok(None);
    };
    let aggregate_end_exclusive = range_end_exclusive.min(cutoff_date.date_naive());
    if range.start_date >= aggregate_end_exclusive {
        return Ok(None);
    }

    let aggregate_start_utc = chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(
        range
            .start_date
            .and_hms_opt(0, 0, 0)
            .expect("midnight should be valid"),
        chrono::Utc,
    );
    let aggregate_end_utc = chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(
        aggregate_end_exclusive
            .and_hms_opt(0, 0, 0)
            .expect("midnight should be valid"),
        chrono::Utc,
    );

    let daily_totals = list_user_dashboard_daily_totals_aggregates(
        &pool,
        aggregate_start_utc,
        aggregate_end_utc,
        user_id,
    )
    .await
    .map_err(|err| {
        build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("{error_context}: {err:?}"),
            false,
        )
    })?;
    let daily_models = list_user_dashboard_daily_model_aggregates(
        &pool,
        aggregate_start_utc,
        aggregate_end_utc,
        user_id,
    )
    .await
    .map_err(|err| {
        build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("{error_context}: {err:?}"),
            false,
        )
    })?;

    let mut by_date =
        std::collections::BTreeMap::<chrono::NaiveDate, DashboardDailyAggregate>::new();
    let mut model_summary = std::collections::BTreeMap::<String, DashboardModelAggregate>::new();
    let mut provider_summary =
        std::collections::BTreeMap::<String, DashboardProviderAggregate>::new();

    for row in &daily_totals {
        dashboard_record_daily_totals_aggregate(&mut by_date, row);
    }
    for row in &daily_models {
        dashboard_record_daily_model_aggregate(&mut by_date, &mut model_summary, row);
    }

    if aggregate_end_exclusive <= range.end_date {
        let raw_range = DashboardDateRange {
            start_date: aggregate_end_exclusive,
            end_date: range.end_date,
            tz_offset_minutes: range.tz_offset_minutes,
        };
        let raw_rows =
            dashboard_daily_breakdown_for_range(state, raw_range, Some(user_id), error_context)
                .await?;
        dashboard_apply_daily_breakdown_rows(
            &raw_rows,
            &mut by_date,
            &mut model_summary,
            &mut provider_summary,
        );
    }

    Ok(Some(dashboard_build_daily_stats_payload(
        range,
        false,
        &by_date,
        &model_summary,
        &provider_summary,
    )))
}

fn dashboard_usage_totals_from_summary(
    summary: &StoredUsageDashboardSummary,
) -> DashboardUsageTotals {
    let mut totals = DashboardUsageTotals::default();
    totals.absorb_summary(summary);
    totals
}

async fn dashboard_load_api_key_counts(
    state: &AppState,
    is_admin: bool,
    user_id: &str,
) -> Result<(u64, u64), GatewayError> {
    let now_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;
    let summary = if is_admin {
        let user_keys = state
            .summarize_auth_api_key_export_non_standalone_records(now_unix_secs)
            .await?;
        let standalone_keys = state
            .summarize_auth_api_key_export_standalone_records(now_unix_secs)
            .await?;
        aether_data::repository::auth::AuthApiKeyExportSummary {
            total: user_keys.total.saturating_add(standalone_keys.total),
            active: user_keys.active.saturating_add(standalone_keys.active),
        }
    } else {
        state
            .summarize_auth_api_key_export_records_by_user_ids(
                &[user_id.to_string()],
                now_unix_secs,
            )
            .await?
    };
    Ok((summary.total, summary.active))
}

async fn dashboard_load_user_counts(
    state: &AppState,
    range: DashboardDateRange,
) -> Result<(u64, u64), GatewayError> {
    let summary = state.summarize_export_users().await?;
    if summary.total > 0 {
        return Ok((summary.total, summary.active));
    }

    let Some((created_from_unix_secs, created_until_unix_secs)) =
        dashboard_range_bounds_unix(range)
    else {
        return Ok((0, 0));
    };
    let fallback = state
        .aggregate_usage_audits(&UsageAuditAggregationQuery {
            created_from_unix_secs,
            created_until_unix_secs,
            group_by: UsageAuditAggregationGroupBy::User,
            limit: 10_000,
        })
        .await?;
    let count = fallback.len() as u64;
    Ok((count, count))
}

fn dashboard_cache_savings_usd(summary: &StoredUsageCostSavingsSummary) -> f64 {
    let estimated_full_cost =
        if summary.estimated_full_cost_usd <= 0.0 && summary.cache_read_cost_usd > 0.0 {
            summary.cache_read_cost_usd * 10.0
        } else {
            summary.estimated_full_cost_usd
        };
    dashboard_round_f64(
        (estimated_full_cost - summary.cache_read_cost_usd).max(0.0),
        4,
    )
}

async fn dashboard_load_cache_savings(
    state: &AppState,
    range: DashboardDateRange,
    user_id: Option<&str>,
) -> Result<f64, GatewayError> {
    let Some((created_from_unix_secs, created_until_unix_secs)) =
        dashboard_range_bounds_unix(range)
    else {
        return Ok(0.0);
    };
    let summary = state
        .summarize_usage_cost_savings(&UsageCostSavingsSummaryQuery {
            created_from_unix_secs,
            created_until_unix_secs,
            user_id: user_id.map(ToOwned::to_owned),
            provider_name: None,
            model: None,
        })
        .await?;
    Ok(dashboard_cache_savings_usd(&summary))
}

pub(super) async fn handle_dashboard_stats_get(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    if !state.has_usage_data_reader() {
        return dashboard_backend_unavailable_response("Usage data backend unavailable");
    }

    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let is_admin = dashboard_role_is_admin(&auth.user.role);

    let cache_identity = if is_admin {
        "admin"
    } else {
        auth.user.id.as_str()
    };
    let query_string = request_context
        .request_query_string
        .as_deref()
        .unwrap_or("");
    let cache_key = format!("stats:{cache_identity}:{query_string}");
    let cache_ttl = std::time::Duration::from_secs(15);

    if let Some(cached) = state.dashboard_response_cache.get(&cache_key, cache_ttl) {
        return Response::builder()
            .status(http::StatusCode::OK)
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(cached))
            .unwrap_or_else(|_| http::StatusCode::INTERNAL_SERVER_ERROR.into_response());
    }

    let query = request_context.request_query_string.as_deref();
    let summary_range = match dashboard_parse_stats_range(query) {
        Ok(value) => value,
        Err(detail) => return dashboard_bad_request_response(detail),
    };
    let today_date = dashboard_user_today(summary_range.tz_offset_minutes);
    let today_range = DashboardDateRange {
        start_date: today_date,
        end_date: today_date,
        tz_offset_minutes: summary_range.tz_offset_minutes,
    };
    let user_filter = (!is_admin).then_some(auth.user.id.as_str());
    let period_summary = match dashboard_summary_for_range(
        state,
        summary_range,
        user_filter,
        "dashboard stats lookup failed",
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    let today_summary = match dashboard_summary_for_range(
        state,
        today_range,
        user_filter,
        "dashboard today stats lookup failed",
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };

    let period_totals = dashboard_usage_totals_from_summary(&period_summary);
    let today_totals = dashboard_usage_totals_from_summary(&today_summary);

    let api_key_counts = match dashboard_load_api_key_counts(state, is_admin, &auth.user.id).await {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("dashboard api key stats lookup failed: {err:?}"),
                false,
            );
        }
    };

    let cache_stats = json!({
        "cache_creation_tokens": period_totals.cache_creation_tokens,
        "cache_read_tokens": period_totals.cache_read_tokens,
        "cache_creation_cost": dashboard_round_f64(period_totals.cache_creation_cost_usd, 4),
        "cache_read_cost": dashboard_round_f64(period_totals.cache_read_cost_usd, 4),
        "cache_hit_rate": period_totals.cache_hit_rate(),
        "total_cache_tokens": period_totals.cache_creation_tokens + period_totals.cache_read_tokens,
    });
    let token_breakdown = json!({
        "input": period_totals.input_tokens,
        "output": period_totals.output_tokens,
        "cache_creation": period_totals.cache_creation_tokens,
        "cache_read": period_totals.cache_read_tokens,
    });
    let today_token_value = dashboard_today_token_value_from_subvalue(&today_totals);
    let today_payload = json!({
        "requests": today_totals.requests,
        "tokens": today_token_value,
        "cost": dashboard_round_f64(today_totals.total_cost_usd, 4),
        "actual_cost": dashboard_round_f64(today_totals.actual_total_cost_usd, 4),
        "cache_creation_tokens": today_totals.cache_creation_tokens,
        "cache_read_tokens": today_totals.cache_read_tokens,
    });

    if is_admin {
        let (total_users, active_users) =
            match dashboard_load_user_counts(state, summary_range).await {
                Ok(value) => value,
                Err(err) => {
                    return build_auth_error_response(
                        http::StatusCode::INTERNAL_SERVER_ERROR,
                        format!("dashboard user stats lookup failed: {err:?}"),
                        false,
                    );
                }
            };
        let success_rate = if today_totals.requests == 0 {
            0.0
        } else {
            (today_totals
                .requests
                .saturating_sub(today_totals.error_requests)) as f64
                / today_totals.requests as f64
                * 100.0
        };
        let cost_savings =
            match dashboard_load_cache_savings(state, summary_range, user_filter).await {
                Ok(value) => value,
                Err(err) => {
                    return build_auth_error_response(
                        http::StatusCode::INTERNAL_SERVER_ERROR,
                        format!("dashboard cache savings lookup failed: {err:?}"),
                        false,
                    );
                }
            };
        let stats = json!([
            {
                "name": "今日请求",
                "value": dashboard_format_integer(today_totals.requests),
                "subValue": format!("成功率 {}", dashboard_format_percentage(success_rate)),
                "icon": "Activity",
            },
            {
                "name": "今日 Token",
                "value": dashboard_format_token_compact(today_token_value),
                "subValue": dashboard_format_today_token_subvalue(&today_totals),
                "icon": "Zap",
            },
            {
                "name": "今日费用",
                "value": dashboard_format_usd(today_totals.total_cost_usd),
                "subValue": format!("节省 {}", dashboard_format_usd(cost_savings.max(0.0))),
                "icon": "DollarSign",
            },
            {
                "name": "活跃用户",
                "value": dashboard_format_integer(active_users),
                "subValue": format!("总用户 {}", dashboard_format_integer(total_users)),
                "icon": "Users",
            }
        ]);
        let payload = json!({
            "stats": stats,
            "today": today_payload,
            "api_keys": {
                "total": api_key_counts.0,
                "active": api_key_counts.1,
            },
            "tokens": {
                "month": period_totals.total_tokens,
            },
            "system_health": {
                "avg_response_time": period_totals.avg_response_time_seconds(),
                "error_rate": if period_totals.requests == 0 { 0.0 } else { dashboard_round_f64(period_totals.error_requests as f64 / period_totals.requests as f64 * 100.0, 4) },
                "error_requests": period_totals.error_requests,
                "fallback_count": 0,
                "total_requests": period_totals.requests,
            },
            "cost_stats": {
                "total_cost": dashboard_round_f64(period_totals.total_cost_usd, 4),
                "total_actual_cost": dashboard_round_f64(period_totals.actual_total_cost_usd, 4),
                "cost_savings": cost_savings,
            },
            "cache_stats": cache_stats,
            "users": {
                "total": total_users,
                "active": active_users,
            },
            "token_breakdown": token_breakdown,
        });
        return dashboard_cached_json_response(state, cache_key, cache_ttl, &payload);
    }

    let wallet = if state.has_wallet_data_reader() {
        match state
            .find_wallet(aether_data::repository::wallet::WalletLookupKey::UserId(
                auth.user.id.as_str(),
            ))
            .await
        {
            Ok(value) => value,
            Err(err) => {
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("dashboard wallet lookup failed: {err:?}"),
                    false,
                );
            }
        }
    } else {
        None
    };
    let wallet_value = wallet
        .as_ref()
        .map(|wallet| {
            if wallet.limit_mode.eq_ignore_ascii_case("unlimited") {
                "无限制".to_string()
            } else {
                dashboard_format_usd(wallet.balance)
            }
        })
        .unwrap_or_else(|| dashboard_format_usd(0.0));
    let wallet_sub_value = wallet
        .as_ref()
        .map(|wallet| {
            if wallet.limit_mode.eq_ignore_ascii_case("unlimited") {
                "无限额度".to_string()
            } else {
                format!("赠款 {}", dashboard_format_usd(wallet.gift_balance))
            }
        })
        .unwrap_or_else(|| "暂无钱包".to_string());
    let payload = json!({
        "stats": [
            {
                "name": "API 密钥",
                "value": dashboard_format_integer(api_key_counts.0),
                "subValue": format!("活跃 {}", dashboard_format_integer(api_key_counts.1)),
                "icon": "Activity",
            },
            {
                "name": "本月请求",
                "value": dashboard_format_integer(period_totals.requests),
                "subValue": format!("今日 {}", dashboard_format_integer(today_totals.requests)),
                "icon": "Users",
            },
            {
                "name": "钱包余额",
                "value": wallet_value,
                "subValue": wallet_sub_value,
                "icon": "DollarSign",
            },
            {
                "name": "本月 Token",
                "value": dashboard_format_integer(period_totals.total_tokens),
                "subValue": dashboard_format_token_subvalue(&period_totals),
                "icon": "Zap",
            }
        ],
        "today": today_payload,
        "api_keys": {
            "total": api_key_counts.0,
            "active": api_key_counts.1,
        },
        "cache_stats": cache_stats,
        "token_breakdown": token_breakdown,
        "monthly_cost": dashboard_round_f64(period_totals.total_cost_usd, 4),
    });
    dashboard_cached_json_response(state, cache_key, cache_ttl, &payload)
}

fn dashboard_daily_aggregate_record(
    aggregate: &mut DashboardDailyAggregate,
    row: &StoredUsageDashboardDailyBreakdownRow,
) {
    aggregate.totals.requests = aggregate.totals.requests.saturating_add(row.requests);
    aggregate.totals.total_tokens = aggregate
        .totals
        .total_tokens
        .saturating_add(row.total_tokens);
    aggregate.totals.total_cost_usd += row.total_cost_usd;
    aggregate.totals.response_time_sum_ms += row.response_time_sum_ms;
    aggregate.totals.response_time_samples = aggregate
        .totals
        .response_time_samples
        .saturating_add(row.response_time_samples);

    let model = aggregate.models.entry(row.model.clone()).or_default();
    model.requests = model.requests.saturating_add(row.requests);
    model.tokens = model.tokens.saturating_add(row.total_tokens);
    model.cost += row.total_cost_usd;
    model.response_time_sum_ms += row.response_time_sum_ms;
    model.response_time_samples = model
        .response_time_samples
        .saturating_add(row.response_time_samples);

    let provider = aggregate.providers.entry(row.provider.clone()).or_default();
    provider.requests = provider.requests.saturating_add(row.requests);
    provider.tokens = provider.tokens.saturating_add(row.total_tokens);
    provider.cost += row.total_cost_usd;
}

pub(super) async fn handle_dashboard_daily_stats_get(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    if !state.has_usage_data_reader() {
        return dashboard_backend_unavailable_response("Usage data backend unavailable");
    }

    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let is_admin = dashboard_role_is_admin(&auth.user.role);

    let cache_identity = if is_admin {
        "admin"
    } else {
        auth.user.id.as_str()
    };
    let query_string = request_context
        .request_query_string
        .as_deref()
        .unwrap_or("");
    let cache_key = format!("daily:{cache_identity}:{query_string}");
    let cache_ttl = std::time::Duration::from_secs(30);

    if let Some(cached) = state.dashboard_response_cache.get(&cache_key, cache_ttl) {
        return Response::builder()
            .status(http::StatusCode::OK)
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(cached))
            .unwrap_or_else(|_| http::StatusCode::INTERNAL_SERVER_ERROR.into_response());
    }

    let query = request_context.request_query_string.as_deref();
    let range = match dashboard_parse_daily_range(query) {
        Ok(value) => value,
        Err(detail) => return dashboard_bad_request_response(detail),
    };
    let user_filter = (!is_admin).then_some(auth.user.id.as_str());
    if is_admin {
        match dashboard_admin_daily_stats_aggregate_payload(
            state,
            range,
            "dashboard daily stats lookup failed",
        )
        .await
        {
            Ok(Some(payload)) => {
                return dashboard_cached_json_response(state, cache_key, cache_ttl, &payload)
            }
            Ok(None) => {}
            Err(response) => return response,
        }
    } else {
        match dashboard_user_daily_stats_aggregate_payload(
            state,
            range,
            auth.user.id.as_str(),
            "dashboard daily stats lookup failed",
        )
        .await
        {
            Ok(Some(payload)) => {
                return dashboard_cached_json_response(state, cache_key, cache_ttl, &payload)
            }
            Ok(None) => {}
            Err(response) => return response,
        }
    }

    let usage = match dashboard_daily_breakdown_for_range(
        state,
        range,
        user_filter,
        "dashboard daily stats lookup failed",
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };

    let mut by_date =
        std::collections::BTreeMap::<chrono::NaiveDate, DashboardDailyAggregate>::new();
    let mut model_summary = std::collections::BTreeMap::<String, DashboardModelAggregate>::new();
    let mut provider_summary =
        std::collections::BTreeMap::<String, DashboardProviderAggregate>::new();
    dashboard_apply_daily_breakdown_rows(
        &usage,
        &mut by_date,
        &mut model_summary,
        &mut provider_summary,
    );

    let payload = dashboard_build_daily_stats_payload(
        range,
        is_admin,
        &by_date,
        &model_summary,
        &provider_summary,
    );
    dashboard_cached_json_response(state, cache_key, cache_ttl, &payload)
}

pub(super) async fn handle_dashboard_recent_requests_get(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    if !state.has_usage_data_reader() {
        return dashboard_backend_unavailable_response("Usage data backend unavailable");
    }

    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let query = request_context.request_query_string.as_deref();
    let limit = match dashboard_parse_limit(query, "limit", 10, 1, 100) {
        Ok(value) => value,
        Err(detail) => return dashboard_bad_request_response(detail),
    };

    let is_admin = dashboard_role_is_admin(&auth.user.role);
    let usage = match state
        .list_recent_usage_audits((!is_admin).then_some(auth.user.id.as_str()), limit)
        .await
    {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("dashboard recent requests lookup failed: {err:?}"),
                false,
            );
        }
    };

    let user_ids = usage
        .iter()
        .filter_map(|item| item.user_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let users_by_id: BTreeMap<String, aether_data::repository::users::StoredUserSummary> =
        match state.resolve_auth_user_summaries_by_ids(&user_ids).await {
            Ok(value) => value,
            Err(err) => {
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("dashboard user lookup failed: {err:?}"),
                    false,
                );
            }
        };
    let mut usernames_by_id: BTreeMap<String, String> = users_by_id
        .iter()
        .filter(|(_, user)| !user.username.trim().is_empty())
        .map(|(user_id, user)| (user_id.clone(), user.username.clone()))
        .collect();

    let requests = usage
        .into_iter()
        .map(|item| {
            let username = item
                .user_id
                .as_ref()
                .and_then(|user_id| usernames_by_id.get(user_id))
                .cloned()
                .or_else(|| {
                    (!state.has_auth_user_data_reader())
                        .then(|| item.username.clone())
                        .flatten()
                })
                .unwrap_or_else(|| "Unknown".to_string());
            json!({
                "id": item.id,
                "user": username,
                "model": dashboard_non_empty_value(&item.model, "N/A"),
                "tokens": item.total_tokens,
                "time": dashboard_format_time_hhmm(item.created_at_unix_ms),
                "is_stream": item.is_stream,
            })
        })
        .collect::<Vec<_>>();

    Json(json!({ "requests": requests })).into_response()
}

pub(super) async fn handle_dashboard_provider_status_get(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    if !state.has_usage_data_reader() {
        return dashboard_backend_unavailable_response("Usage data backend unavailable");
    }
    if !state.has_provider_catalog_data_reader() {
        return dashboard_backend_unavailable_response("Provider catalog backend unavailable");
    }

    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };

    let cache_identity = if dashboard_role_is_admin(&auth.user.role) {
        "admin"
    } else {
        auth.user.id.as_str()
    };
    let cache_key = format!("provider:{cache_identity}");
    let cache_ttl = std::time::Duration::from_secs(20);

    if let Some(cached) = state.dashboard_response_cache.get(&cache_key, cache_ttl) {
        return Response::builder()
            .status(http::StatusCode::OK)
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(cached))
            .unwrap_or_else(|_| http::StatusCode::INTERNAL_SERVER_ERROR.into_response());
    }

    let providers = match state.list_provider_catalog_providers(true).await {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("dashboard provider catalog lookup failed: {err:?}"),
                false,
            );
        }
    };
    let since_unix_secs = u64::try_from(chrono::Utc::now().timestamp())
        .unwrap_or_default()
        .saturating_sub(24 * 3600);
    let now_unix_secs = u64::try_from(chrono::Utc::now().timestamp()).unwrap_or_default();
    let usage = match state
        .summarize_dashboard_provider_counts(&UsageDashboardProviderCountsQuery {
            created_from_unix_secs: since_unix_secs,
            created_until_unix_secs: now_unix_secs,
            user_id: None,
        })
        .await
    {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("dashboard provider usage lookup failed: {err:?}"),
                false,
            );
        }
    };

    let mut request_counts = BTreeMap::<String, u64>::new();
    for item in usage {
        *request_counts
            .entry(item.provider_name.to_ascii_lowercase())
            .or_default() += item.request_count;
    }

    let mut entries = providers
        .into_iter()
        .map(|provider| {
            let request_count = request_counts
                .get(&provider.name.to_ascii_lowercase())
                .copied()
                .unwrap_or(0);
            json!({
                "name": provider.name,
                "status": if provider.is_active { "active" } else { "inactive" },
                "requests": request_count,
            })
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        right["requests"]
            .as_u64()
            .unwrap_or(0)
            .cmp(&left["requests"].as_u64().unwrap_or(0))
            .then_with(|| {
                left["name"]
                    .as_str()
                    .unwrap_or_default()
                    .cmp(right["name"].as_str().unwrap_or_default())
            })
    });
    let limit = if dashboard_role_is_admin(&auth.user.role) {
        10
    } else {
        5
    };
    if entries.len() > limit {
        entries.truncate(limit);
    }

    let payload = json!({ "providers": entries });
    dashboard_cached_json_response(state, cache_key, cache_ttl, &payload)
}

fn dashboard_parse_limit(
    query: Option<&str>,
    field: &str,
    default: usize,
    min: usize,
    max: usize,
) -> Result<usize, String> {
    match query_param_value(query, field) {
        Some(value) => {
            let parsed = value
                .parse::<usize>()
                .map_err(|_| format!("{field} must be an integer between {min} and {max}"))?;
            if (min..=max).contains(&parsed) {
                Ok(parsed)
            } else {
                Err(format!(
                    "{field} must be an integer between {min} and {max}"
                ))
            }
        }
        None => Ok(default),
    }
}

fn dashboard_role_is_admin(role: &str) -> bool {
    role.eq_ignore_ascii_case("admin")
}

fn dashboard_bad_request_response(detail: String) -> Response<Body> {
    (
        http::StatusCode::BAD_REQUEST,
        Json(json!({ "detail": detail })),
    )
        .into_response()
}

fn dashboard_cached_json_response(
    state: &AppState,
    cache_key: String,
    cache_ttl: std::time::Duration,
    payload: &serde_json::Value,
) -> Response<Body> {
    let bytes = serde_json::to_vec(payload).unwrap_or_default();
    state
        .dashboard_response_cache
        .insert(cache_key, bytes.clone(), cache_ttl);
    Response::builder()
        .status(http::StatusCode::OK)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(bytes))
        .unwrap_or_else(|_| http::StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn dashboard_backend_unavailable_response(detail: &'static str) -> Response<Body> {
    (
        http::StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "detail": detail })),
    )
        .into_response()
}

fn dashboard_non_empty_value(value: &str, default: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        default.to_string()
    } else {
        trimmed.to_string()
    }
}

fn dashboard_format_time_hhmm(unix_secs: u64) -> Option<String> {
    let timestamp = i64::try_from(unix_secs).ok()?;
    let datetime = chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp, 0)?;
    Some(datetime.format("%H:%M").to_string())
}
