use super::super::super::support::AdminProviderOpsCheckinOutcome;
use super::super::super::verify::admin_provider_ops_value_as_f64;
use super::super::support::{
    admin_provider_ops_balance_data, admin_provider_ops_parse_rfc3339_unix_secs,
    admin_provider_ops_quota_divisor,
};
use serde_json::json;

pub(super) fn admin_provider_ops_new_api_balance_payload(
    action_config: &serde_json::Map<String, serde_json::Value>,
    response_json: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let user_data = if response_json
        .get("success")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
        && response_json
            .get("data")
            .is_some_and(serde_json::Value::is_object)
    {
        response_json.get("data")
    } else if response_json
        .get("success")
        .and_then(serde_json::Value::as_bool)
        == Some(false)
    {
        return Err(response_json
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("业务状态码表示失败")
            .to_string());
    } else {
        Some(response_json)
    };
    let Some(user_data) = user_data.and_then(serde_json::Value::as_object) else {
        return Err("响应格式无效".to_string());
    };
    let quota_divisor = admin_provider_ops_quota_divisor(action_config);
    let total_available =
        admin_provider_ops_value_as_f64(user_data.get("quota")).map(|value| value / quota_divisor);
    let total_used = admin_provider_ops_value_as_f64(user_data.get("used_quota"))
        .map(|value| value / quota_divisor);
    Ok(admin_provider_ops_balance_data(
        None,
        total_used,
        total_available,
        action_config
            .get("currency")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("USD"),
        serde_json::Map::new(),
    ))
}

pub(super) fn admin_provider_ops_cubence_balance_payload(
    action_config: &serde_json::Map<String, serde_json::Value>,
    response_json: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let response_data = response_json
        .get("data")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "响应格式无效".to_string())?;
    let balance_data = response_data
        .get("balance")
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    let subscription_limits = response_data
        .get("subscription_limits")
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut extra = serde_json::Map::new();
    if let Some(five_hour) = subscription_limits
        .get("five_hour")
        .and_then(serde_json::Value::as_object)
    {
        extra.insert(
            "five_hour_limit".to_string(),
            json!({
                "limit": five_hour.get("limit"),
                "used": five_hour.get("used"),
                "remaining": five_hour.get("remaining"),
                "resets_at": five_hour.get("resets_at"),
            }),
        );
    }
    if let Some(weekly) = subscription_limits
        .get("weekly")
        .and_then(serde_json::Value::as_object)
    {
        extra.insert(
            "weekly_limit".to_string(),
            json!({
                "limit": weekly.get("limit"),
                "used": weekly.get("used"),
                "remaining": weekly.get("remaining"),
                "resets_at": weekly.get("resets_at"),
            }),
        );
    }
    for key in [
        "normal_balance_dollar",
        "subscription_balance_dollar",
        "charity_balance_dollar",
    ] {
        if let Some(value) = balance_data.get(key) {
            extra.insert(
                key.trim_end_matches("_dollar").replace("_dollar", ""),
                value.clone(),
            );
        }
    }
    if let Some(value) = balance_data.get("normal_balance_dollar") {
        extra.insert("normal_balance".to_string(), value.clone());
    }
    if let Some(value) = balance_data.get("subscription_balance_dollar") {
        extra.insert("subscription_balance".to_string(), value.clone());
    }
    if let Some(value) = balance_data.get("charity_balance_dollar") {
        extra.insert("charity_balance".to_string(), value.clone());
    }
    Ok(admin_provider_ops_balance_data(
        None,
        None,
        admin_provider_ops_value_as_f64(balance_data.get("total_balance_dollar")),
        action_config
            .get("currency")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("USD"),
        extra,
    ))
}

pub(super) fn admin_provider_ops_nekocode_balance_payload(
    response_json: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let response_data = response_json
        .get("data")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "响应格式无效".to_string())?;
    let subscription = response_data
        .get("subscription")
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    let balance = admin_provider_ops_value_as_f64(response_data.get("balance"));
    let daily_quota_limit = admin_provider_ops_value_as_f64(subscription.get("daily_quota_limit"));
    let daily_remaining_quota =
        admin_provider_ops_value_as_f64(subscription.get("daily_remaining_quota"));
    let daily_used = match (daily_quota_limit, daily_remaining_quota) {
        (Some(limit), Some(remaining)) => Some(limit - remaining),
        _ => None,
    };
    let mut extra = serde_json::Map::new();
    for key in [
        "plan_name",
        "status",
        "daily_quota_limit",
        "daily_remaining_quota",
        "effective_start_date",
        "effective_end_date",
    ] {
        if let Some(value) = subscription.get(key) {
            extra.insert(
                match key {
                    "status" => "subscription_status",
                    other => other,
                }
                .to_string(),
                value.clone(),
            );
        }
    }
    if let Some(value) = daily_used {
        extra.insert("daily_used_quota".to_string(), json!(value));
    }
    if let Some(month_data) = response_data
        .get("month")
        .and_then(serde_json::Value::as_object)
    {
        extra.insert(
            "month_stats".to_string(),
            json!({
                "total_input_tokens": month_data.get("total_input_tokens"),
                "total_output_tokens": month_data.get("total_output_tokens"),
                "total_quota": month_data.get("total_quota"),
                "total_requests": month_data.get("total_requests"),
            }),
        );
    }
    if let Some(today_data) = response_data
        .get("today")
        .and_then(serde_json::Value::as_object)
    {
        if let Some(stats) = today_data.get("stats") {
            extra.insert("today_stats".to_string(), stats.clone());
        }
    }
    Ok(admin_provider_ops_balance_data(
        daily_quota_limit,
        daily_used,
        balance,
        "USD",
        extra,
    ))
}

pub(super) fn admin_provider_ops_yescode_balance_extra(
    combined_data: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    let pay_as_you_go =
        admin_provider_ops_value_as_f64(combined_data.get("pay_as_you_go_balance")).unwrap_or(0.0);
    let subscription =
        admin_provider_ops_value_as_f64(combined_data.get("subscription_balance")).unwrap_or(0.0);
    let plan = combined_data
        .get("subscription_plan")
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    let daily_balance =
        admin_provider_ops_value_as_f64(plan.get("daily_balance")).unwrap_or(subscription);
    let weekly_limit = admin_provider_ops_value_as_f64(
        combined_data
            .get("weekly_limit")
            .or_else(|| plan.get("weekly_limit")),
    );
    let weekly_spent =
        admin_provider_ops_value_as_f64(combined_data.get("weekly_spent_balance")).unwrap_or(0.0);
    let subscription_available = weekly_limit
        .map(|limit| (limit - weekly_spent).max(0.0).min(subscription))
        .unwrap_or(subscription);

    let mut extra = serde_json::Map::new();
    extra.insert("pay_as_you_go_balance".to_string(), json!(pay_as_you_go));
    extra.insert("daily_limit".to_string(), json!(daily_balance));
    if let Some(limit) = weekly_limit {
        extra.insert("weekly_limit".to_string(), json!(limit));
    }
    extra.insert("weekly_spent".to_string(), json!(weekly_spent));
    if let Some(last_week_reset) =
        admin_provider_ops_parse_rfc3339_unix_secs(combined_data.get("last_week_reset"))
    {
        extra.insert(
            "weekly_resets_at".to_string(),
            json!(last_week_reset + 7 * 24 * 3600),
        );
    }
    if let Some(last_daily_add) =
        admin_provider_ops_parse_rfc3339_unix_secs(combined_data.get("last_daily_balance_add"))
    {
        extra.insert(
            "daily_resets_at".to_string(),
            json!(last_daily_add + 24 * 3600),
        );
    }
    let daily_spent = if let Some(limit) = weekly_limit {
        daily_balance - daily_balance.min(subscription_available.min(limit.max(0.0)))
    } else {
        (daily_balance - subscription).max(0.0)
    };
    extra.insert("daily_spent".to_string(), json!(daily_spent));
    extra.insert(
        "_subscription_available".to_string(),
        json!(subscription_available),
    );
    extra.insert(
        "_total_available".to_string(),
        json!(pay_as_you_go + subscription_available),
    );
    extra
}

pub(super) fn admin_provider_ops_attach_balance_checkin_outcome(
    action_payload: &mut serde_json::Value,
    outcome: &AdminProviderOpsCheckinOutcome,
) {
    if let Some(data) = action_payload
        .get_mut("data")
        .and_then(serde_json::Value::as_object_mut)
    {
        let extra = data
            .entry("extra".to_string())
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        if let Some(extra) = extra.as_object_mut() {
            if outcome.cookie_expired {
                extra.insert("cookie_expired".to_string(), serde_json::Value::Bool(true));
                extra.insert(
                    "cookie_expired_message".to_string(),
                    serde_json::Value::String(outcome.message.clone()),
                );
            } else {
                extra.insert(
                    "checkin_success".to_string(),
                    outcome
                        .success
                        .map(serde_json::Value::Bool)
                        .unwrap_or(serde_json::Value::Null),
                );
                extra.insert(
                    "checkin_message".to_string(),
                    serde_json::Value::String(outcome.message.clone()),
                );
            }
        }
    }
    if outcome.cookie_expired {
        if let Some(object) = action_payload.as_object_mut() {
            object.insert("status".to_string(), json!("auth_expired"));
        }
    }
}
