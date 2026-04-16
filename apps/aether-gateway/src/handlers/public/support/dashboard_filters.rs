use super::{
    build_auth_error_response, query_param_value, resolve_authenticated_local_user, AppState,
    GatewayError, GatewayPublicRequestContext,
};
use aether_billing::{
    normalize_input_tokens_for_billing, normalize_total_input_context_for_cache_hit_rate,
};
use aether_data_contracts::repository::usage::{StoredRequestUsageAudit, UsageAuditListQuery};
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use chrono::Datelike;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

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
    response_time_sum_ms: u64,
    response_time_samples: u64,
}

#[derive(Debug, Default, Clone)]
struct DashboardModelAggregate {
    requests: u64,
    tokens: u64,
    cost: f64,
    response_time_sum_ms: u64,
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
    fn record(&mut self, item: &StoredRequestUsageAudit) {
        let cache_creation_tokens = dashboard_cache_creation_tokens(item);
        self.requests += 1;
        self.input_tokens += item.input_tokens;
        self.effective_input_tokens += dashboard_effective_input_tokens(item);
        self.output_tokens += item.output_tokens;
        self.total_tokens += item.total_tokens;
        self.cache_creation_tokens += cache_creation_tokens;
        self.cache_read_tokens += item.cache_read_input_tokens;
        self.cache_hit_total_input_context += dashboard_total_input_context(item);
        self.cache_creation_cost_usd += item.cache_creation_cost_usd;
        self.cache_read_cost_usd += item.cache_read_cost_usd;
        self.total_cost_usd += item.total_cost_usd;
        self.actual_total_cost_usd += item.actual_total_cost_usd;
        if item.status_code.is_some_and(|value| value >= 400)
            || item.status.eq_ignore_ascii_case("failed")
        {
            self.error_requests += 1;
        }
        if let Some(response_time_ms) = item.response_time_ms {
            self.response_time_sum_ms += response_time_ms;
            self.response_time_samples += 1;
        }
    }

    fn avg_response_time_seconds(&self) -> f64 {
        if self.response_time_samples == 0 {
            0.0
        } else {
            dashboard_round_f64(
                (self.response_time_sum_ms as f64 / self.response_time_samples as f64) / 1000.0,
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

fn dashboard_usage_should_count_in_summary(item: &StoredRequestUsageAudit) -> bool {
    !matches!(item.status.as_str(), "pending" | "streaming")
        && !matches!(item.provider_name.as_str(), "unknown" | "pending")
}

fn dashboard_cache_creation_tokens(item: &StoredRequestUsageAudit) -> u64 {
    let classified = item
        .cache_creation_ephemeral_5m_input_tokens
        .saturating_add(item.cache_creation_ephemeral_1h_input_tokens);
    if item.cache_creation_input_tokens == 0 && classified > 0 {
        classified
    } else {
        item.cache_creation_input_tokens
    }
}

fn dashboard_total_input_context(item: &StoredRequestUsageAudit) -> u64 {
    let api_format = item
        .endpoint_api_format
        .as_deref()
        .or(item.api_format.as_deref());
    let input_tokens = i64::try_from(item.input_tokens).unwrap_or(i64::MAX);
    let cache_creation_tokens =
        i64::try_from(dashboard_cache_creation_tokens(item)).unwrap_or(i64::MAX);
    let cache_read_tokens = i64::try_from(item.cache_read_input_tokens).unwrap_or(i64::MAX);
    normalize_total_input_context_for_cache_hit_rate(
        api_format,
        input_tokens,
        cache_creation_tokens,
        cache_read_tokens,
    ) as u64
}

fn dashboard_effective_input_tokens(item: &StoredRequestUsageAudit) -> u64 {
    let api_format = item
        .endpoint_api_format
        .as_deref()
        .or(item.api_format.as_deref());
    let input_tokens = i64::try_from(item.input_tokens).unwrap_or(i64::MAX);
    let cache_read_tokens = i64::try_from(item.cache_read_input_tokens).unwrap_or(i64::MAX);
    normalize_input_tokens_for_billing(api_format, input_tokens, cache_read_tokens) as u64
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
    let end_local = range.end_date.and_hms_opt(23, 59, 59)?;
    let start_utc = (start_local - offset).and_utc().timestamp();
    let end_utc = (end_local - offset).and_utc().timestamp();
    Some((start_utc.max(0) as u64, end_utc.max(0) as u64))
}

fn dashboard_usage_local_date(
    item: &StoredRequestUsageAudit,
    tz_offset_minutes: i32,
) -> Option<chrono::NaiveDate> {
    let timestamp = i64::try_from(item.created_at_unix_ms).ok()?;
    let datetime = chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp, 0)?;
    Some((datetime + chrono::Duration::minutes(i64::from(tz_offset_minutes))).date_naive())
}

async fn dashboard_list_usage_for_range(
    state: &AppState,
    range: DashboardDateRange,
    user_id: Option<&str>,
    error_context: &str,
) -> Result<Vec<StoredRequestUsageAudit>, Response<Body>> {
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
        .list_usage_audits(&UsageAuditListQuery {
            created_from_unix_secs: Some(created_from_unix_secs),
            created_until_unix_secs: Some(created_until_unix_secs),
            user_id: user_id.map(ToOwned::to_owned),
            provider_name: None,
            model: None,
            api_format: None,
            statuses: None,
            is_stream: None,
            error_only: false,
            limit: None,
            offset: None,
            newest_first: false,
        })
        .await
    {
        Ok(mut value) => {
            value.retain(dashboard_usage_should_count_in_summary);
            Ok(value)
        }
        Err(err) => Err(build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("{error_context}: {err:?}"),
            false,
        )),
    }
}

fn dashboard_usage_totals(usage: &[StoredRequestUsageAudit]) -> DashboardUsageTotals {
    let mut totals = DashboardUsageTotals::default();
    for item in usage {
        totals.record(item);
    }
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
    fallback_user_ids: &[String],
) -> Result<(u64, u64), GatewayError> {
    let summary = state.summarize_export_users().await?;
    if summary.total > 0 {
        return Ok((summary.total, summary.active));
    }
    let unique = fallback_user_ids
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    Ok((unique.len() as u64, unique.len() as u64))
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
    let period_usage = match dashboard_list_usage_for_range(
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
    let today_usage = match dashboard_list_usage_for_range(
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

    let period_totals = dashboard_usage_totals(&period_usage);
    let today_totals = dashboard_usage_totals(&today_usage);
    let fallback_user_ids = period_usage
        .iter()
        .filter_map(|item| item.user_id.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

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
    let today_payload = json!({
        "requests": today_totals.requests,
        "tokens": today_totals.total_tokens,
        "cost": dashboard_round_f64(today_totals.total_cost_usd, 4),
        "actual_cost": dashboard_round_f64(today_totals.actual_total_cost_usd, 4),
        "cache_creation_tokens": today_totals.cache_creation_tokens,
        "cache_read_tokens": today_totals.cache_read_tokens,
    });

    if is_admin {
        let (total_users, active_users) =
            match dashboard_load_user_counts(state, &fallback_user_ids).await {
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
        let cost_savings = dashboard_round_f64(
            period_totals.total_cost_usd - period_totals.actual_total_cost_usd,
            4,
        );
        let stats = json!([
            {
                "name": "今日请求",
                "value": dashboard_format_integer(today_totals.requests),
                "subValue": format!("成功率 {}", dashboard_format_percentage(success_rate)),
                "icon": "Activity",
            },
            {
                "name": "今日 Token",
                "value": dashboard_format_token_compact(today_totals.total_tokens),
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
    item: &StoredRequestUsageAudit,
) {
    aggregate.totals.record(item);
    let model = aggregate.models.entry(item.model.clone()).or_default();
    model.requests += 1;
    model.tokens += item.total_tokens;
    model.cost += item.total_cost_usd;
    if let Some(response_time_ms) = item.response_time_ms {
        model.response_time_sum_ms += response_time_ms;
        model.response_time_samples += 1;
    }

    let provider = aggregate
        .providers
        .entry(item.provider_name.clone())
        .or_default();
    provider.requests += 1;
    provider.tokens += item.total_tokens;
    provider.cost += item.total_cost_usd;
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
    let usage = match dashboard_list_usage_for_range(
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

    for item in &usage {
        let Some(date) = dashboard_usage_local_date(item, range.tz_offset_minutes) else {
            continue;
        };
        let aggregate = by_date.entry(date).or_default();
        dashboard_daily_aggregate_record(aggregate, item);

        let model = model_summary.entry(item.model.clone()).or_default();
        model.requests += 1;
        model.tokens += item.total_tokens;
        model.cost += item.total_cost_usd;
        if let Some(response_time_ms) = item.response_time_ms {
            model.response_time_sum_ms += response_time_ms;
            model.response_time_samples += 1;
        }

        let provider = provider_summary
            .entry(item.provider_name.clone())
            .or_default();
        provider.requests += 1;
        provider.tokens += item.total_tokens;
        provider.cost += item.total_cost_usd;
    }

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
                    (value.response_time_sum_ms as f64 / value.response_time_samples as f64)
                        / 1000.0,
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
    let usage = match state
        .list_usage_audits(&UsageAuditListQuery {
            created_from_unix_secs: Some(since_unix_secs),
            created_until_unix_secs: None,
            user_id: None,
            provider_name: None,
            model: None,
            api_format: None,
            statuses: None,
            is_stream: None,
            error_only: false,
            limit: None,
            offset: None,
            newest_first: false,
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
            .or_default() += 1;
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
