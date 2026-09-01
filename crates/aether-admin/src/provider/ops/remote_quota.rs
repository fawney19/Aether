use std::collections::BTreeMap;

use serde_json::{Map, Value};

const DEFAULT_PROGRESS_ENDPOINT: &str = "/api/v1/subscriptions/progress";
const DAY_SECONDS: u64 = 24 * 60 * 60;
const MAX_FUTURE_WINDOW_START_SKEW_SECONDS: u64 = 5 * 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sub2ApiRemoteQuotaConfig {
    pub group_id: String,
    pub progress_endpoint: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Sub2ApiQuotaWindowKind {
    Daily,
    Weekly,
    Monthly,
}

impl Sub2ApiQuotaWindowKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
        }
    }

    pub fn interval_days(self) -> u64 {
        match self {
            Self::Daily => 1,
            Self::Weekly => 7,
            Self::Monthly => 30,
        }
    }

    fn display_name_zh(self) -> &'static str {
        match self {
            Self::Daily => "日",
            Self::Weekly => "周",
            Self::Monthly => "月",
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Sub2ApiRemoteQuotaGroup {
    pub group_id: String,
    pub group_name: String,
    pub subscription_id: String,
    pub daily_limit_usd: f64,
    pub daily_used_usd: f64,
    pub weekly_limit_usd: f64,
    pub weekly_used_usd: f64,
    pub monthly_limit_usd: f64,
    pub monthly_used_usd: f64,
    pub local_sync_window: Option<Sub2ApiQuotaWindowKind>,
    pub expires_at_unix_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Sub2ApiRemoteQuotaSnapshot {
    ActiveLimited {
        group_id: String,
        group_name: String,
        subscription_id: String,
        window: Sub2ApiQuotaWindowKind,
        limit_usd: f64,
        used_usd: f64,
        window_start_unix_secs: u64,
        resets_at_unix_secs: u64,
        expires_at_unix_secs: Option<u64>,
    },
    ActiveUnlimited {
        group_id: String,
        group_name: String,
        subscription_id: String,
        expires_at_unix_secs: Option<u64>,
    },
    Exhausted {
        group_id: String,
    },
}

#[derive(Debug, Clone)]
struct SummarySubscription {
    id: String,
    group_id: String,
    group_name: String,
    status: String,
    daily_limit_usd: f64,
    daily_used_usd: f64,
    weekly_limit_usd: f64,
    weekly_used_usd: f64,
    monthly_limit_usd: f64,
    monthly_used_usd: f64,
    expires_at_unix_secs: Option<u64>,
}

impl SummarySubscription {
    fn is_active_at(&self, now_unix_secs: u64) -> bool {
        self.status.eq_ignore_ascii_case("active")
            && self
                .expires_at_unix_secs
                .is_none_or(|expires_at| expires_at > now_unix_secs)
    }

    fn limited_windows(&self) -> impl Iterator<Item = (Sub2ApiQuotaWindowKind, f64, f64)> {
        [
            (
                Sub2ApiQuotaWindowKind::Daily,
                self.daily_limit_usd,
                self.daily_used_usd,
            ),
            (
                Sub2ApiQuotaWindowKind::Weekly,
                self.weekly_limit_usd,
                self.weekly_used_usd,
            ),
            (
                Sub2ApiQuotaWindowKind::Monthly,
                self.monthly_limit_usd,
                self.monthly_used_usd,
            ),
        ]
        .into_iter()
        .filter(|(_, limit_usd, _)| *limit_usd > 0.0)
    }

    fn local_sync_window(&self) -> Option<(Sub2ApiQuotaWindowKind, f64, f64)> {
        // Every upstream request consumes all active windows. The window with
        // the least remaining USD is therefore the binding local constraint.
        self.limited_windows().min_by(|left, right| {
            remaining_usd(left.1, left.2).total_cmp(&remaining_usd(right.1, right.2))
        })
    }
}

#[derive(Debug, Clone)]
struct ProgressWindow {
    limit_usd: f64,
    used_usd: f64,
    window_start_unix_secs: u64,
    resets_at_unix_secs: u64,
}

#[derive(Debug, Clone)]
struct ProgressSubscription {
    subscription_id: String,
    group_id: String,
    daily: Option<ProgressWindow>,
    weekly: Option<ProgressWindow>,
    monthly: Option<ProgressWindow>,
    expires_at_unix_secs: Option<u64>,
}

impl ProgressSubscription {
    fn window(&self, kind: Sub2ApiQuotaWindowKind) -> Option<&ProgressWindow> {
        match kind {
            Sub2ApiQuotaWindowKind::Daily => self.daily.as_ref(),
            Sub2ApiQuotaWindowKind::Weekly => self.weekly.as_ref(),
            Sub2ApiQuotaWindowKind::Monthly => self.monthly.as_ref(),
        }
    }
}

pub fn parse_sub2api_remote_quota_config(
    provider_ops_config: &Map<String, Value>,
) -> Result<Option<Sub2ApiRemoteQuotaConfig>, String> {
    let Some(remote_quota) = provider_ops_config.get("remote_quota") else {
        return Ok(None);
    };
    let remote_quota = remote_quota
        .as_object()
        .ok_or_else(|| "remote_quota 必须是 JSON 对象".to_string())?;
    if remote_quota.get("enabled").and_then(Value::as_bool) != Some(true) {
        return Ok(None);
    }
    match remote_quota.get("sync_mode") {
        None => {}
        Some(Value::String(mode)) if mode == "overwrite_local_quota" => {}
        _ => return Err("remote_quota.sync_mode 必须是 overwrite_local_quota".to_string()),
    }

    let group_id = canonical_json_id(remote_quota.get("group_id"))
        .ok_or_else(|| "remote_quota.group_id 不能为空".to_string())?;
    let progress_endpoint = remote_quota
        .get("progress_endpoint")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_PROGRESS_ENDPOINT);
    validate_sub2api_same_origin_endpoint(progress_endpoint)?;

    Ok(Some(Sub2ApiRemoteQuotaConfig {
        group_id,
        progress_endpoint: progress_endpoint.to_string(),
    }))
}

pub fn validate_sub2api_same_origin_endpoint(endpoint: &str) -> Result<(), String> {
    let endpoint = endpoint.trim();
    if !endpoint.starts_with('/')
        || endpoint.starts_with("//")
        || endpoint.contains('#')
        || endpoint.contains('\\')
    {
        return Err("Sub2API 配额端点必须是同源、以 / 开头且不含 fragment 的相对路径".to_string());
    }
    Ok(())
}

pub fn parse_sub2api_remote_quota_groups(
    subscription_json: &Value,
) -> Result<Vec<Sub2ApiRemoteQuotaGroup>, String> {
    let now_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;
    parse_sub2api_remote_quota_groups_at(subscription_json, now_unix_secs)
}

fn parse_sub2api_remote_quota_groups_at(
    subscription_json: &Value,
    now_unix_secs: u64,
) -> Result<Vec<Sub2ApiRemoteQuotaGroup>, String> {
    let mut groups = BTreeMap::<String, Sub2ApiRemoteQuotaGroup>::new();
    for subscription in parse_summary_subscriptions(subscription_json)? {
        if !subscription.is_active_at(now_unix_secs) {
            continue;
        }
        let local_sync_window = subscription
            .local_sync_window()
            .map(|(window, _, _)| window);
        let group = Sub2ApiRemoteQuotaGroup {
            group_id: subscription.group_id.clone(),
            group_name: subscription.group_name,
            subscription_id: subscription.id,
            daily_limit_usd: subscription.daily_limit_usd,
            daily_used_usd: subscription.daily_used_usd,
            weekly_limit_usd: subscription.weekly_limit_usd,
            weekly_used_usd: subscription.weekly_used_usd,
            monthly_limit_usd: subscription.monthly_limit_usd,
            monthly_used_usd: subscription.monthly_used_usd,
            local_sync_window,
            expires_at_unix_secs: subscription.expires_at_unix_secs,
        };
        if groups
            .insert(subscription.group_id.clone(), group)
            .is_some()
        {
            return Err(format!(
                "Sub2API Group {} 存在多个活跃套餐，无法确定唯一额度",
                subscription.group_id
            ));
        }
    }
    Ok(groups.into_values().collect())
}

pub fn parse_sub2api_remote_quota(
    subscription_json: &Value,
    progress_json: Option<&Value>,
    expected_group_id: &str,
) -> Result<Sub2ApiRemoteQuotaSnapshot, String> {
    let now_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;
    parse_sub2api_remote_quota_at(
        subscription_json,
        progress_json,
        expected_group_id,
        now_unix_secs,
    )
}

fn parse_sub2api_remote_quota_at(
    subscription_json: &Value,
    progress_json: Option<&Value>,
    expected_group_id: &str,
    now_unix_secs: u64,
) -> Result<Sub2ApiRemoteQuotaSnapshot, String> {
    let expected_group_id = expected_group_id.trim();
    if expected_group_id.is_empty() {
        return Err("remote_quota.group_id 不能为空".to_string());
    }

    let subscriptions = parse_summary_subscriptions(subscription_json)?;
    let mut matching = subscriptions
        .into_iter()
        .filter(|subscription| {
            subscription.group_id == expected_group_id && subscription.is_active_at(now_unix_secs)
        })
        .collect::<Vec<_>>();
    if matching.is_empty() {
        return Ok(Sub2ApiRemoteQuotaSnapshot::Exhausted {
            group_id: expected_group_id.to_string(),
        });
    }
    if matching.len() != 1 {
        return Err(format!(
            "Sub2API Group {expected_group_id} 存在多个活跃套餐，无法确定唯一额度"
        ));
    }
    let subscription = matching.remove(0);

    let limited_windows = subscription.limited_windows().collect::<Vec<_>>();
    if limited_windows.is_empty() {
        return Ok(Sub2ApiRemoteQuotaSnapshot::ActiveUnlimited {
            group_id: subscription.group_id,
            group_name: subscription.group_name,
            subscription_id: subscription.id,
            expires_at_unix_secs: subscription.expires_at_unix_secs,
        });
    }

    let progress_json = progress_json.ok_or_else(|| {
        format!(
            "Sub2API Group {} 的有限额度同步需要 subscriptions/progress 响应",
            subscription.group_id
        )
    })?;
    let progresses = parse_progress_subscriptions(progress_json)?;
    let mut matching_progress = progresses.into_iter().filter(|progress| {
        progress.subscription_id == subscription.id && progress.group_id == subscription.group_id
    });
    let progress = matching_progress.next().ok_or_else(|| {
        format!(
            "Sub2API Group {} 的有限额度 progress 数据缺失",
            subscription.group_id
        )
    })?;
    if matching_progress.next().is_some() {
        return Err(format!(
            "Sub2API Group {} 存在多个匹配的 progress 记录，无法确定唯一额度窗口",
            subscription.group_id
        ));
    }
    let (window, summary_limit_usd, progress_window) = limited_windows
        .into_iter()
        .map(|(window, summary_limit_usd, _)| {
            validate_progress_window(
                &subscription,
                &progress,
                window,
                summary_limit_usd,
                now_unix_secs,
            )
            .map(|progress_window| (window, summary_limit_usd, progress_window))
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .min_by(|left, right| {
            remaining_usd(left.1, left.2.used_usd)
                .total_cmp(&remaining_usd(right.1, right.2.used_usd))
        })
        .expect("finite quota has at least one validated progress window");

    Ok(Sub2ApiRemoteQuotaSnapshot::ActiveLimited {
        group_id: subscription.group_id,
        group_name: subscription.group_name,
        subscription_id: subscription.id,
        window,
        limit_usd: summary_limit_usd,
        // Only progress binds usage to the selected window. Summary is fetched
        // independently and its usage may still belong to the previous window.
        used_usd: progress_window.used_usd,
        window_start_unix_secs: progress_window.window_start_unix_secs,
        resets_at_unix_secs: progress_window.resets_at_unix_secs,
        // Summary owns subscription identity and expiry; progress only enriches
        // the selected quota window and usage.
        expires_at_unix_secs: subscription
            .expires_at_unix_secs
            .or(progress.expires_at_unix_secs),
    })
}

fn validate_progress_window<'a>(
    subscription: &SummarySubscription,
    progress: &'a ProgressSubscription,
    window: Sub2ApiQuotaWindowKind,
    summary_limit_usd: f64,
    now_unix_secs: u64,
) -> Result<&'a ProgressWindow, String> {
    let progress_window = progress.window(window).ok_or_else(|| {
        format!(
            "Sub2API Group {} 已配置{}额度，但 progress.{} 没有返回当前额度窗口。通常是该订阅尚未完成过成功的 API 请求，或上一窗口到期后新窗口尚未激活。请先用该订阅成功调用一次 API，或在 Sub2API 管理端重置{}额度，再重新同步。本次未修改 Aether 本地额度。",
            subscription.group_id,
            window.display_name_zh(),
            window.as_str(),
            window.display_name_zh(),
        )
    })?;
    if progress_window.window_start_unix_secs
        > now_unix_secs.saturating_add(MAX_FUTURE_WINDOW_START_SKEW_SECONDS)
        || progress_window.resets_at_unix_secs <= now_unix_secs
    {
        return Err(format!(
            "Sub2API Group {} 的{}额度 progress 窗口不覆盖当前时间",
            subscription.group_id,
            window.as_str()
        ));
    }
    let limit_tolerance = summary_limit_usd.abs().max(1.0) * 1e-9;
    if (progress_window.limit_usd - summary_limit_usd).abs() > limit_tolerance {
        return Err(format!(
            "Sub2API Group {} 的{}额度在 summary 与 progress 中不一致",
            subscription.group_id,
            window.as_str()
        ));
    }
    Ok(progress_window)
}

fn remaining_usd(limit_usd: f64, used_usd: f64) -> f64 {
    (limit_usd - used_usd).max(0.0)
}

fn parse_summary_subscriptions(payload: &Value) -> Result<Vec<SummarySubscription>, String> {
    let data = successful_envelope_data(payload, "Sub2API 套餐摘要")?;
    let items = if let Some(items) = data.as_array() {
        items
    } else {
        data.as_object()
            .and_then(|data| data.get("subscriptions"))
            .and_then(Value::as_array)
            .ok_or_else(|| "Sub2API 套餐摘要缺少 subscriptions 数组".to_string())?
    };
    items
        .iter()
        .map(parse_summary_subscription)
        .collect::<Result<Vec<_>, _>>()
}

fn parse_summary_subscription(value: &Value) -> Result<SummarySubscription, String> {
    let item = value
        .as_object()
        .ok_or_else(|| "Sub2API 套餐元素必须是 JSON 对象".to_string())?;
    let id = canonical_json_id(item.get("id")).ok_or_else(|| "Sub2API 套餐缺少 id".to_string())?;
    let group_id = canonical_json_id(item.get("group_id"))
        .or_else(|| {
            item.get("group")
                .and_then(Value::as_object)
                .and_then(|group| canonical_json_id(group.get("id")))
        })
        .ok_or_else(|| format!("Sub2API 套餐 {id} 缺少 group_id"))?;
    let group = item.get("group").and_then(Value::as_object);
    let status = item
        .get("status")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("Sub2API 套餐 {id} 缺少 status"))?;
    let group_name = item
        .get("group_name")
        .and_then(Value::as_str)
        .or_else(|| {
            group
                .and_then(|group| group.get("name"))
                .and_then(Value::as_str)
        })
        .unwrap_or_default()
        .trim()
        .to_string();
    let daily_limit_usd = summary_limit_usd(item, group, "daily")?;
    let daily_used_usd = summary_used_usd(item, "daily")?;
    let weekly_limit_usd = summary_limit_usd(item, group, "weekly")?;
    let weekly_used_usd = summary_used_usd(item, "weekly")?;
    let monthly_limit_usd = summary_limit_usd(item, group, "monthly")?;
    let monthly_used_usd = summary_used_usd(item, "monthly")?;

    Ok(SummarySubscription {
        id,
        group_id,
        group_name,
        status,
        daily_limit_usd,
        daily_used_usd,
        weekly_limit_usd,
        weekly_used_usd,
        monthly_limit_usd,
        monthly_used_usd,
        expires_at_unix_secs: optional_rfc3339_unix_secs(item.get("expires_at"), "expires_at")?,
    })
}

fn summary_limit_usd(
    item: &Map<String, Value>,
    group: Option<&Map<String, Value>>,
    window: &str,
) -> Result<f64, String> {
    let field = format!("{window}_limit_usd");
    optional_non_negative_f64(
        item.get(&field)
            .filter(|value| !value.is_null())
            .or_else(|| {
                group
                    .and_then(|group| group.get(&field))
                    .filter(|value| !value.is_null())
            }),
        &field,
    )
    .map(|value| value.unwrap_or(0.0))
}

fn summary_used_usd(item: &Map<String, Value>, window: &str) -> Result<f64, String> {
    let used_field = format!("{window}_used_usd");
    let usage_field = format!("{window}_usage_usd");
    optional_non_negative_f64(
        item.get(&used_field)
            .filter(|value| !value.is_null())
            .or_else(|| item.get(&usage_field).filter(|value| !value.is_null())),
        &used_field,
    )
    .map(|value| value.unwrap_or(0.0))
}

fn parse_progress_subscriptions(payload: &Value) -> Result<Vec<ProgressSubscription>, String> {
    let items = successful_envelope_data(payload, "Sub2API 套餐进度")?
        .as_array()
        .ok_or_else(|| "Sub2API 套餐进度 data 必须是数组".to_string())?;
    items
        .iter()
        .map(parse_progress_subscription)
        .collect::<Result<Vec<_>, _>>()
}

fn parse_progress_subscription(value: &Value) -> Result<ProgressSubscription, String> {
    let item = value
        .as_object()
        .ok_or_else(|| "Sub2API 套餐进度元素必须是 JSON 对象".to_string())?;
    let subscription = item
        .get("subscription")
        .and_then(Value::as_object)
        .ok_or_else(|| "Sub2API 套餐进度缺少 subscription".to_string())?;
    let progress = item
        .get("progress")
        .and_then(Value::as_object)
        .ok_or_else(|| "Sub2API 套餐进度缺少 progress".to_string())?;
    let subscription_id = canonical_json_id(subscription.get("id"))
        .or_else(|| canonical_json_id(progress.get("id")))
        .ok_or_else(|| "Sub2API 套餐进度缺少 subscription.id".to_string())?;
    let group_id = canonical_json_id(subscription.get("group_id"))
        .or_else(|| {
            subscription
                .get("group")
                .and_then(Value::as_object)
                .and_then(|group| canonical_json_id(group.get("id")))
        })
        .ok_or_else(|| format!("Sub2API 套餐进度 {subscription_id} 缺少 group_id"))?;

    Ok(ProgressSubscription {
        subscription_id,
        group_id,
        daily: parse_progress_window(progress, Sub2ApiQuotaWindowKind::Daily)?,
        weekly: parse_progress_window(progress, Sub2ApiQuotaWindowKind::Weekly)?,
        monthly: parse_progress_window(progress, Sub2ApiQuotaWindowKind::Monthly)?,
        expires_at_unix_secs: optional_rfc3339_unix_secs(
            progress
                .get("expires_at")
                .or_else(|| subscription.get("expires_at")),
            "expires_at",
        )?,
    })
}

fn parse_progress_window(
    progress: &Map<String, Value>,
    kind: Sub2ApiQuotaWindowKind,
) -> Result<Option<ProgressWindow>, String> {
    let Some(value) = progress.get(kind.as_str()).filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let window = value
        .as_object()
        .ok_or_else(|| format!("Sub2API {} progress 必须是 JSON 对象", kind.as_str()))?;
    let prefix = kind.as_str();
    let limit_usd = required_positive_f64(window.get("limit_usd"), &format!("{prefix}.limit_usd"))?;
    let used_usd =
        optional_non_negative_f64(window.get("used_usd"), &format!("{prefix}.used_usd"))?
            .unwrap_or(0.0);
    let window_start_unix_secs = required_rfc3339_unix_secs(
        window.get("window_start"),
        &format!("{prefix}.window_start"),
    )?;
    let resets_at_unix_secs =
        optional_rfc3339_unix_secs(window.get("resets_at"), &format!("{prefix}.resets_at"))?
            .unwrap_or_else(|| {
                window_start_unix_secs
                    .saturating_add(kind.interval_days().saturating_mul(DAY_SECONDS))
            });
    if resets_at_unix_secs <= window_start_unix_secs {
        return Err(format!("Sub2API {prefix}.resets_at 必须晚于 window_start"));
    }

    Ok(Some(ProgressWindow {
        limit_usd,
        used_usd,
        window_start_unix_secs,
        resets_at_unix_secs,
    }))
}

fn successful_envelope_data<'a>(payload: &'a Value, label: &str) -> Result<&'a Value, String> {
    let object = payload
        .as_object()
        .ok_or_else(|| format!("{label}响应必须是 JSON 对象"))?;
    if object.get("code").and_then(Value::as_i64) != Some(0) {
        return Err(object
            .get("message")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|message| !message.is_empty())
            .unwrap_or("业务状态码表示失败")
            .to_string());
    }
    object
        .get("data")
        .ok_or_else(|| format!("{label}响应缺少 data"))
}

fn canonical_json_id(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(value) => {
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_string())
        }
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn value_as_f64(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|value| value as f64))
        .or_else(|| value.as_u64().map(|value| value as f64))
        .or_else(|| value.as_str()?.trim().parse::<f64>().ok())
}

fn optional_non_negative_f64(value: Option<&Value>, field: &str) -> Result<Option<f64>, String> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let value = value_as_f64(value).ok_or_else(|| format!("Sub2API {field} 必须是数字"))?;
    if !value.is_finite() || value < 0.0 {
        return Err(format!("Sub2API {field} 必须是有限的非负数"));
    }
    Ok(Some(value))
}

fn required_positive_f64(value: Option<&Value>, field: &str) -> Result<f64, String> {
    let value = optional_non_negative_f64(value, field)?
        .ok_or_else(|| format!("Sub2API {field} 不能为空"))?;
    if value <= 0.0 {
        return Err(format!("Sub2API {field} 必须大于 0"));
    }
    Ok(value)
}

fn optional_rfc3339_unix_secs(value: Option<&Value>, field: &str) -> Result<Option<u64>, String> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let raw = value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("Sub2API {field} 必须是 RFC3339 时间"))?;
    let timestamp = chrono::DateTime::parse_from_rfc3339(raw)
        .map_err(|_| format!("Sub2API {field} 必须是 RFC3339 时间"))?
        .timestamp();
    u64::try_from(timestamp)
        .map(Some)
        .map_err(|_| format!("Sub2API {field} 不能早于 Unix epoch"))
}

fn required_rfc3339_unix_secs(value: Option<&Value>, field: &str) -> Result<u64, String> {
    optional_rfc3339_unix_secs(value, field)?.ok_or_else(|| format!("Sub2API {field} 不能为空"))
}

#[cfg(test)]
mod tests {
    use super::{
        parse_sub2api_remote_quota_at, parse_sub2api_remote_quota_config,
        parse_sub2api_remote_quota_groups_at, validate_sub2api_same_origin_endpoint,
        Sub2ApiQuotaWindowKind, Sub2ApiRemoteQuotaGroup, Sub2ApiRemoteQuotaSnapshot,
    };
    use serde_json::{json, Value};

    const TEST_NOW_UNIX_SECS: u64 = 1_896_004_800; // 2030-01-30T12:00:00Z

    fn parse_sub2api_remote_quota_groups(
        subscription_json: &Value,
    ) -> Result<Vec<Sub2ApiRemoteQuotaGroup>, String> {
        parse_sub2api_remote_quota_groups_at(subscription_json, TEST_NOW_UNIX_SECS)
    }

    fn parse_sub2api_remote_quota(
        subscription_json: &Value,
        progress_json: Option<&Value>,
        expected_group_id: &str,
    ) -> Result<Sub2ApiRemoteQuotaSnapshot, String> {
        parse_sub2api_remote_quota_at(
            subscription_json,
            progress_json,
            expected_group_id,
            TEST_NOW_UNIX_SECS,
        )
    }

    fn summary() -> serde_json::Value {
        json!({
            "code": 0,
            "data": {
                "subscriptions": [{
                    "id": 9,
                    "group_id": 42,
                    "group_name": "Pro",
                    "status": "active",
                    "monthly_used_usd": 12.5,
                    "monthly_limit_usd": 100,
                    "expires_at": "2030-02-01T00:00:00Z"
                }]
            }
        })
    }

    fn progress() -> serde_json::Value {
        json!({
            "code": 0,
            "data": [{
                "subscription": {
                    "id": 9,
                    "group_id": 42,
                    "monthly_usage_usd": 12.0,
                    "monthly_window_start": "2030-01-01T00:00:00Z"
                },
                "progress": {
                    "id": 9,
                    "expires_at": "2030-02-01T00:00:00Z",
                    "monthly": {
                        "limit_usd": 100,
                        "used_usd": 12.0,
                        "window_start": "2030-01-01T00:00:00Z",
                        "resets_at": "2030-01-31T00:00:00Z"
                    }
                }
            }]
        })
    }

    #[test]
    fn lists_only_active_remote_quota_groups() {
        let groups = parse_sub2api_remote_quota_groups(&json!({
            "code": 0,
            "data": {
                "subscriptions": [
                    {
                        "id": 9,
                        "group_id": 42,
                        "group_name": "Pro",
                        "status": "active",
                        "daily_used_usd": 1.5,
                        "daily_limit_usd": 10,
                        "weekly_used_usd": 4.5,
                        "weekly_limit_usd": 50,
                        "monthly_used_usd": 12.5,
                        "monthly_limit_usd": 100,
                        "expires_at": "2030-02-01T00:00:00Z"
                    },
                    {
                        "id": 10,
                        "group_id": 7,
                        "group_name": "Expired",
                        "status": "expired",
                        "monthly_limit_usd": 20
                    },
                    {
                        "id": 11,
                        "group_id": 8,
                        "group_name": "Past active record",
                        "status": "active",
                        "monthly_limit_usd": 20,
                        "expires_at": "2020-01-01T00:00:00Z"
                    }
                ]
            }
        }))
        .expect("group catalog should parse");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].group_id, "42");
        assert_eq!(groups[0].group_name, "Pro");
        assert_eq!(groups[0].daily_limit_usd, 10.0);
        assert_eq!(groups[0].daily_used_usd, 1.5);
        assert_eq!(groups[0].weekly_limit_usd, 50.0);
        assert_eq!(groups[0].weekly_used_usd, 4.5);
        assert_eq!(groups[0].monthly_limit_usd, 100.0);
        assert_eq!(groups[0].monthly_used_usd, 12.5);
        assert_eq!(
            groups[0].local_sync_window,
            Some(Sub2ApiQuotaWindowKind::Daily)
        );
    }

    #[test]
    fn falls_back_to_nested_limits_and_usage_aliases_when_flat_fields_are_null() {
        let summary = json!({
            "code": 0,
            "data": {"subscriptions": [{
                "id": 9,
                "group_id": 42,
                "status": "active",
                "daily_limit_usd": null,
                "daily_used_usd": null,
                "daily_usage_usd": 2.5,
                "group": {"daily_limit_usd": 10}
            }]}
        });

        let groups = parse_sub2api_remote_quota_groups(&summary)
            .expect("null flat fields should fall back to the nested contract");
        assert_eq!(groups[0].daily_limit_usd, 10.0);
        assert_eq!(groups[0].daily_used_usd, 2.5);
    }

    #[test]
    fn summary_expiry_remains_authoritative_over_progress_enrichment() {
        let summary = json!({
            "code": 0,
            "data": {"subscriptions": [{
                "id": 9,
                "group_id": 42,
                "status": "active",
                "monthly_limit_usd": 100,
                "monthly_used_usd": 12,
                "expires_at": "2030-02-01T00:00:00Z"
            }]}
        });
        let progress = json!({
            "code": 0,
            "data": [{
                "subscription": {"id": 9, "group_id": 42},
                "progress": {
                    "expires_at": "2040-02-01T00:00:00Z",
                    "monthly": {
                        "limit_usd": 100,
                        "used_usd": 12,
                        "window_start": "2030-01-01T00:00:00Z",
                        "resets_at": "2030-01-31T00:00:00Z"
                    }
                }
            }]
        });

        let parsed = parse_sub2api_remote_quota(&summary, Some(&progress), "42")
            .expect("summary and progress should parse");
        let Sub2ApiRemoteQuotaSnapshot::ActiveLimited {
            expires_at_unix_secs,
            ..
        } = parsed
        else {
            panic!("expected a finite quota window");
        };
        assert_eq!(expires_at_unix_secs, Some(1_896_134_400));
    }

    #[test]
    fn selects_the_finite_window_with_the_least_remaining_quota() {
        let summary = json!({
            "code": 0,
            "data": {"subscriptions": [{
                "id": 9,
                "group_id": 42,
                "group_name": "Pro",
                "status": "active",
                "daily_usage_usd": 2,
                "weekly_used_usd": 8,
                "monthly_used_usd": 20,
                "group": {
                    "id": 42,
                    "daily_limit_usd": 10,
                    "weekly_limit_usd": 50,
                    "monthly_limit_usd": 0
                }
            }]}
        });
        let progress = json!({
            "code": 0,
            "data": [{
                "subscription": {"id": 9, "group_id": 42},
                "progress": {
                    "id": 9,
                    "daily": {
                        "limit_usd": 10,
                        "used_usd": 1.5,
                        "window_start": "2030-01-30T00:00:00Z",
                        "resets_at": "2030-01-31T00:00:00Z"
                    },
                    "weekly": {
                        "limit_usd": 50,
                        "used_usd": 50,
                        "window_start": "2030-01-24T00:00:00Z",
                        "resets_at": "2030-01-31T00:00:00Z"
                    }
                }
            }]
        });

        let parsed = parse_sub2api_remote_quota(&summary, Some(&progress), "42")
            .expect("multi-window quota should parse");
        let Sub2ApiRemoteQuotaSnapshot::ActiveLimited {
            window,
            limit_usd,
            used_usd,
            window_start_unix_secs,
            resets_at_unix_secs,
            ..
        } = parsed
        else {
            panic!("expected a finite quota window");
        };
        assert_eq!(window, Sub2ApiQuotaWindowKind::Weekly);
        assert_eq!(limit_usd, 50.0);
        assert_eq!(used_usd, 50.0);
        assert_eq!(resets_at_unix_secs - window_start_unix_secs, 604_800);
    }

    #[test]
    fn progress_usage_owns_the_selected_window_at_rollover() {
        let mut old_summary = summary();
        old_summary["data"]["subscriptions"][0]["monthly_used_usd"] = json!(100.0);
        let mut new_progress = progress();
        new_progress["data"][0]["progress"]["monthly"]["used_usd"] = json!(0.0);

        let parsed = parse_sub2api_remote_quota(&old_summary, Some(&new_progress), "42")
            .expect("rollover responses should parse");
        assert!(matches!(
            parsed,
            Sub2ApiRemoteQuotaSnapshot::ActiveLimited { used_usd: 0.0, .. }
        ));
    }

    #[test]
    fn selects_weekly_window_when_daily_is_unlimited() {
        let summary = json!({
            "code": 0,
            "data": {"subscriptions": [{
                "id": 9,
                "group_id": 42,
                "status": "active",
                "daily_limit_usd": 0,
                "weekly_limit_usd": 50,
                "weekly_used_usd": 8,
                "monthly_limit_usd": 100,
                "monthly_used_usd": 20
            }]}
        });
        let progress = json!({
            "code": 0,
            "data": [{
                "subscription": {"id": 9, "group_id": 42},
                "progress": {
                    "weekly": {
                        "limit_usd": 50,
                        "used_usd": 7,
                        "window_start": "2030-01-24T00:00:00Z",
                        "resets_at": "2030-01-31T00:00:00Z"
                    },
                    "monthly": {
                        "limit_usd": 100,
                        "used_usd": 19,
                        "window_start": "2030-01-01T00:00:00Z",
                        "resets_at": "2030-01-31T00:00:00Z"
                    }
                }
            }]
        });

        let parsed = parse_sub2api_remote_quota(&summary, Some(&progress), "42")
            .expect("weekly quota should parse");
        assert!(matches!(
            parsed,
            Sub2ApiRemoteQuotaSnapshot::ActiveLimited {
                window: Sub2ApiQuotaWindowKind::Weekly,
                limit_usd: 50.0,
                used_usd: 7.0,
                ..
            }
        ));
    }

    #[test]
    fn explains_how_to_activate_a_missing_weekly_progress_window() {
        let summary = json!({
            "code": 0,
            "data": {"subscriptions": [{
                "id": 9,
                "group_id": 33,
                "status": "active",
                "weekly_limit_usd": 50
            }]}
        });
        let progress = json!({
            "code": 0,
            "data": [{
                "subscription": {"id": 9, "group_id": 33},
                "progress": {}
            }]
        });

        let error = parse_sub2api_remote_quota(&summary, Some(&progress), "33")
            .expect_err("a finite quota without its progress window must fail closed");
        assert_eq!(
            error,
            "Sub2API Group 33 已配置周额度，但 progress.weekly 没有返回当前额度窗口。通常是该订阅尚未完成过成功的 API 请求，或上一窗口到期后新窗口尚未激活。请先用该订阅成功调用一次 API，或在 Sub2API 管理端重置周额度，再重新同步。本次未修改 Aether 本地额度。"
        );
    }

    #[test]
    fn parses_selected_group_monthly_window() {
        let parsed = parse_sub2api_remote_quota(&summary(), Some(&progress()), "42")
            .expect("remote quota should parse");
        assert_eq!(
            parsed,
            Sub2ApiRemoteQuotaSnapshot::ActiveLimited {
                group_id: "42".to_string(),
                group_name: "Pro".to_string(),
                subscription_id: "9".to_string(),
                window: Sub2ApiQuotaWindowKind::Monthly,
                limit_usd: 100.0,
                used_usd: 12.0,
                window_start_unix_secs: 1_893_456_000,
                resets_at_unix_secs: 1_896_048_000,
                expires_at_unix_secs: Some(1_896_134_400),
            }
        );
    }

    #[test]
    fn rejects_duplicate_active_subscriptions_for_one_group() {
        let summary = json!({
            "code": 0,
            "data": {
                "subscriptions": [
                    {
                        "id": 9,
                        "group_id": 42,
                        "group_name": "Pro",
                        "status": "active",
                        "monthly_limit_usd": 100
                    },
                    {
                        "id": 10,
                        "group_id": "42",
                        "group_name": "Pro renewal",
                        "status": "active",
                        "monthly_limit_usd": 100
                    }
                ]
            }
        });

        let error = parse_sub2api_remote_quota_groups(&summary)
            .expect_err("duplicate active subscriptions must be rejected");
        assert!(error.contains("多个活跃套餐"));
    }

    #[test]
    fn treats_zero_or_missing_limit_as_unlimited() {
        let parsed = parse_sub2api_remote_quota(
            &json!({
                "code": 0,
                "data": {"subscriptions": [{
                    "id": "sub-1",
                    "group_id": "group-1",
                    "status": "active"
                }]}
            }),
            None,
            "group-1",
        )
        .expect("unlimited quota should parse");
        assert!(matches!(
            parsed,
            Sub2ApiRemoteQuotaSnapshot::ActiveUnlimited { .. }
        ));
    }

    #[test]
    fn classifies_missing_selected_group_as_exhausted() {
        let parsed = parse_sub2api_remote_quota(&summary(), Some(&progress()), "99")
            .expect("empty selected group should classify");
        assert_eq!(
            parsed,
            Sub2ApiRemoteQuotaSnapshot::Exhausted {
                group_id: "99".to_string()
            }
        );
    }

    #[test]
    fn treats_expired_active_record_as_unavailable() {
        let summary = json!({
            "code": 0,
            "data": {"subscriptions": [{
                "id": 9,
                "group_id": 42,
                "status": "active",
                "monthly_limit_usd": 100,
                "expires_at": "2020-01-01T00:00:00Z"
            }]}
        });

        let groups = parse_sub2api_remote_quota_groups(&summary)
            .expect("expired subscription should parse as an empty catalog");
        assert!(groups.is_empty());
        assert_eq!(
            parse_sub2api_remote_quota(&summary, None, "42")
                .expect("expired selected group should classify"),
            Sub2ApiRemoteQuotaSnapshot::Exhausted {
                group_id: "42".to_string()
            }
        );
    }

    #[test]
    fn rejects_incomplete_authority_item_instead_of_dropping_it() {
        let error = parse_sub2api_remote_quota(
            &json!({"code": 0, "data": {"subscriptions": [{"group_id": 42}]}}),
            None,
            "42",
        )
        .expect_err("missing id must fail");
        assert!(error.contains("缺少 id"));
    }

    #[test]
    fn rejects_authority_item_without_status_instead_of_classifying_exhausted() {
        let error = parse_sub2api_remote_quota(
            &json!({
                "code": 0,
                "data": {"subscriptions": [{"id": 9, "group_id": 42}]}
            }),
            None,
            "42",
        )
        .expect_err("missing status must fail the authoritative snapshot");
        assert!(error.contains("缺少 status"));
    }

    #[test]
    fn rejects_duplicate_matching_progress_records() {
        let mut duplicate_progress = progress();
        let duplicate = duplicate_progress["data"][0].clone();
        duplicate_progress["data"]
            .as_array_mut()
            .expect("progress data should be an array")
            .push(duplicate);

        let error = parse_sub2api_remote_quota(&summary(), Some(&duplicate_progress), "42")
            .expect_err("duplicate matching progress must not be selected by response order");
        assert!(error.contains("多个匹配的 progress"));
    }

    #[test]
    fn requires_progress_for_limited_monthly_quota() {
        let error = parse_sub2api_remote_quota(&summary(), None, "42")
            .expect_err("limited quota needs progress");
        assert!(error.contains("subscriptions/progress"));
    }

    #[test]
    fn rejects_stale_or_future_progress_windows() {
        let mut stale = progress();
        stale["data"][0]["progress"]["monthly"]["window_start"] = json!("2030-01-01T00:00:00Z");
        stale["data"][0]["progress"]["monthly"]["resets_at"] = json!("2030-01-30T00:00:00Z");
        let stale_error = parse_sub2api_remote_quota(&summary(), Some(&stale), "42")
            .expect_err("an expired progress window must not mutate local quota");
        assert!(stale_error.contains("不覆盖当前时间"));

        let mut future = progress();
        future["data"][0]["progress"]["monthly"]["window_start"] = json!("2030-01-31T00:00:00Z");
        future["data"][0]["progress"]["monthly"]["resets_at"] = json!("2030-03-02T00:00:00Z");
        let future_error = parse_sub2api_remote_quota(&summary(), Some(&future), "42")
            .expect_err("a future progress window must not poison the local window fence");
        assert!(future_error.contains("不覆盖当前时间"));
    }

    #[test]
    fn parses_and_validates_remote_quota_config() {
        let config = parse_sub2api_remote_quota_config(
            json!({
                "remote_quota": {
                    "enabled": true,
                    "group_id": 42,
                    "progress_endpoint": "/api/v1/subscriptions/progress"
                }
            })
            .as_object()
            .expect("config"),
        )
        .expect("config should parse")
        .expect("config should be enabled");
        assert_eq!(config.group_id, "42");
        let explicit_legacy = parse_sub2api_remote_quota_config(
            json!({
                "remote_quota": {
                    "enabled": true,
                    "sync_mode": "overwrite_local_quota",
                    "group_id": "42"
                }
            })
            .as_object()
            .expect("config"),
        )
        .expect("explicit legacy mode should parse");
        assert!(explicit_legacy.is_some());
        assert!(parse_sub2api_remote_quota_config(
            json!({
                "remote_quota": {
                    "enabled": true,
                    "sync_mode": "key_group_pools"
                }
            })
            .as_object()
            .expect("config"),
        )
        .is_err());
        assert!(validate_sub2api_same_origin_endpoint("/api/v1/subscriptions/summary").is_ok());
        assert!(validate_sub2api_same_origin_endpoint("https://evil.example/steal").is_err());
        assert!(validate_sub2api_same_origin_endpoint("//evil.example/steal").is_err());
    }
}
