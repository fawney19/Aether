use super::verify::admin_provider_ops_value_as_f64;
use serde_json::{json, Map, Value};

#[derive(Debug, Clone)]
pub struct ProviderOpsCheckinOutcome {
    pub success: Option<bool>,
    pub message: String,
    pub cookie_expired: bool,
}

pub fn parse_query_balance_payload(
    architecture_id: &str,
    action_config: &Map<String, Value>,
    response_json: &Value,
) -> Result<Value, String> {
    match architecture_id {
        "generic_api" | "new_api" | "anyrouter" | "done_hub" => {
            parse_new_api_balance_payload(action_config, response_json)
        }
        "usage_api" => parse_usage_api_balance_payload(action_config, response_json),
        "cubence" => parse_cubence_balance_payload(action_config, response_json),
        "nekocode" => parse_nekocode_balance_payload(response_json),
        _ => Err("Provider 操作仅支持 Rust execution runtime".to_string()),
    }
}

pub fn parse_yescode_combined_balance_payload(
    action_config: &Map<String, Value>,
    combined_data: &Map<String, Value>,
) -> Value {
    let mut extra = yescode_balance_extra(combined_data);
    let total_available = admin_provider_ops_value_as_f64(extra.get("_total_available"));
    extra.remove("_subscription_available");
    extra.remove("_total_available");
    build_balance_data(
        None,
        None,
        total_available,
        action_config
            .get("currency")
            .and_then(Value::as_str)
            .unwrap_or("USD"),
        extra,
    )
}

pub fn parse_sub2api_balance_payload(
    action_config: &Map<String, Value>,
    me_json: &Value,
    subscription_json: Option<&Value>,
) -> Result<Value, String> {
    let Some(me_payload) = me_json.as_object() else {
        return Err("响应格式无效".to_string());
    };
    if me_payload.get("code").and_then(Value::as_i64).unwrap_or(-1) != 0 {
        return Err("查询用户信息失败".to_string());
    }
    let Some(me_data) = me_payload.get("data").and_then(Value::as_object) else {
        return Err("响应格式无效".to_string());
    };

    let balance = value_as_f64(me_data.get("balance")).unwrap_or(0.0);
    let points = value_as_f64(me_data.get("points")).unwrap_or(0.0);
    let mut extra = Map::new();
    extra.insert("balance".to_string(), json!(balance));
    extra.insert("points".to_string(), json!(points));

    if let Some(subscription_json) = subscription_json {
        if let Some(subscription_payload) = subscription_json.as_object() {
            if subscription_payload
                .get("code")
                .and_then(Value::as_i64)
                .unwrap_or(-1)
                == 0
            {
                if let Some(summary) = subscription_payload.get("data").and_then(Value::as_object) {
                    if let Some(active_count) = summary.get("active_count") {
                        extra.insert("active_subscriptions".to_string(), active_count.clone());
                    }
                    if let Some(total_used_usd) = summary.get("total_used_usd") {
                        extra.insert("total_used_usd".to_string(), total_used_usd.clone());
                    }
                    if let Some(subscriptions) =
                        summary.get("subscriptions").and_then(Value::as_array)
                    {
                        extra.insert(
                            "subscriptions".to_string(),
                            Value::Array(
                                subscriptions
                                    .iter()
                                    .filter_map(parse_subscription)
                                    .collect(),
                            ),
                        );
                    }
                }
            }
        }
    }

    Ok(build_balance_data(
        None,
        None,
        Some(balance + points),
        action_config
            .get("currency")
            .and_then(Value::as_str)
            .unwrap_or("USD"),
        extra,
    ))
}

pub fn attach_balance_checkin_outcome(
    action_payload: &mut Value,
    outcome: &ProviderOpsCheckinOutcome,
) {
    if let Some(data) = action_payload
        .get_mut("data")
        .and_then(Value::as_object_mut)
    {
        let extra = data
            .entry("extra".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(extra) = extra.as_object_mut() {
            let message = stable_checkin_outcome_message(outcome);
            if outcome.cookie_expired {
                extra.insert("cookie_expired".to_string(), Value::Bool(true));
                extra.insert(
                    "cookie_expired_message".to_string(),
                    Value::String(message.to_string()),
                );
            } else {
                extra.insert(
                    "checkin_success".to_string(),
                    outcome.success.map(Value::Bool).unwrap_or(Value::Null),
                );
                extra.insert(
                    "checkin_message".to_string(),
                    Value::String(message.to_string()),
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

fn stable_checkin_outcome_message(outcome: &ProviderOpsCheckinOutcome) -> &'static str {
    if outcome.cookie_expired {
        return "Cookie 已失效";
    }
    match outcome.success {
        Some(true) => "签到成功",
        Some(false) => "签到失败",
        None => "今日已签到",
    }
}

pub fn build_balance_data(
    total_granted: Option<f64>,
    total_used: Option<f64>,
    total_available: Option<f64>,
    currency: &str,
    extra: Map<String, Value>,
) -> Value {
    json!({
        "total_granted": total_granted,
        "total_used": total_used,
        "total_available": total_available,
        "expires_at": Value::Null,
        "currency": currency,
        "extra": extra,
    })
}

fn parse_new_api_balance_payload(
    action_config: &Map<String, Value>,
    response_json: &Value,
) -> Result<Value, String> {
    let user_data = if response_json.get("success").and_then(Value::as_bool) == Some(true)
        && response_json.get("data").is_some_and(Value::is_object)
    {
        response_json.get("data")
    } else if response_json.get("success").and_then(Value::as_bool) == Some(false) {
        return Err("业务状态码表示失败".to_string());
    } else {
        Some(response_json)
    };
    let Some(user_data) = user_data.and_then(Value::as_object) else {
        return Err("响应格式无效".to_string());
    };
    let quota_divisor = quota_divisor(action_config);
    let total_available =
        admin_provider_ops_value_as_f64(user_data.get("quota")).map(|value| value / quota_divisor);
    let total_used = admin_provider_ops_value_as_f64(user_data.get("used_quota"))
        .map(|value| value / quota_divisor);
    Ok(build_balance_data(
        None,
        total_used,
        total_available,
        action_config
            .get("currency")
            .and_then(Value::as_str)
            .unwrap_or("USD"),
        Map::new(),
    ))
}

fn parse_usage_api_balance_payload(
    action_config: &Map<String, Value>,
    response_json: &Value,
) -> Result<Value, String> {
    let data = response_json
        .as_object()
        .ok_or_else(|| "响应格式无效".to_string())?;
    let is_valid = data
        .get("is_active")
        .and_then(Value::as_bool)
        .or_else(|| data.get("isValid").and_then(Value::as_bool));
    if is_valid == Some(false) {
        return Err("API Key 无效或已停用".to_string());
    }

    let quota = data.get("quota").and_then(Value::as_object);
    let remaining = admin_provider_ops_value_as_f64(data.get("remaining"))
        .or_else(|| quota.and_then(|quota| admin_provider_ops_value_as_f64(quota.get("remaining"))))
        .or_else(|| admin_provider_ops_value_as_f64(data.get("balance")))
        .ok_or_else(|| "响应缺少余额字段".to_string())?;
    let currency = data
        .get("unit")
        .and_then(Value::as_str)
        .or_else(|| quota.and_then(|quota| quota.get("unit").and_then(Value::as_str)))
        .or_else(|| action_config.get("currency").and_then(Value::as_str))
        .unwrap_or("USD");

    let mut extra = Map::new();
    extra.insert(
        "is_valid".to_string(),
        Value::Bool(is_valid.unwrap_or(true)),
    );
    if let Some(value) = data.get("balance") {
        extra.insert("balance".to_string(), value.clone());
    }
    if let Some(value) = data.get("planName").or_else(|| data.get("plan_name")) {
        extra.insert("plan_name".to_string(), value.clone());
    }
    if let Some(value) = data.get("mode") {
        extra.insert("mode".to_string(), value.clone());
    }

    Ok(build_balance_data(
        None,
        None,
        Some(remaining),
        currency,
        extra,
    ))
}

fn parse_cubence_balance_payload(
    action_config: &Map<String, Value>,
    response_json: &Value,
) -> Result<Value, String> {
    let response_data = if response_json.get("success").and_then(Value::as_bool) == Some(true)
        && response_json.get("data").is_some_and(Value::is_object)
    {
        response_json.get("data")
    } else if response_json.get("success").and_then(Value::as_bool) == Some(false) {
        return Err("查询余额失败".to_string());
    } else {
        Some(response_json)
    };
    let response_data = response_data
        .and_then(Value::as_object)
        .ok_or_else(|| "响应格式无效".to_string())?;
    let balance_data = response_data
        .get("balance")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let subscription_limits = response_data
        .get("subscription_limits")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut extra = Map::new();
    if let Some(five_hour) = subscription_limits
        .get("five_hour")
        .and_then(Value::as_object)
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
    if let Some(weekly) = subscription_limits.get("weekly").and_then(Value::as_object) {
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
    if let Some(value) = balance_data.get("normal_balance_dollar") {
        extra.insert("normal_balance".to_string(), value.clone());
    }
    if let Some(value) = balance_data.get("subscription_balance_dollar") {
        extra.insert("subscription_balance".to_string(), value.clone());
    }
    if let Some(value) = balance_data.get("charity_balance_dollar") {
        extra.insert("charity_balance".to_string(), value.clone());
    }
    Ok(build_balance_data(
        None,
        None,
        admin_provider_ops_value_as_f64(balance_data.get("total_balance_dollar")),
        action_config
            .get("currency")
            .and_then(Value::as_str)
            .unwrap_or("USD"),
        extra,
    ))
}

fn parse_nekocode_balance_payload(response_json: &Value) -> Result<Value, String> {
    let response_data = response_json
        .get("data")
        .and_then(Value::as_object)
        .ok_or_else(|| "响应格式无效".to_string())?;
    let subscription = response_data
        .get("subscription")
        .and_then(Value::as_object)
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
    let mut extra = Map::new();
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
    if let Some(month_data) = response_data.get("month").and_then(Value::as_object) {
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
    if let Some(today_data) = response_data.get("today").and_then(Value::as_object) {
        if let Some(stats) = today_data.get("stats") {
            extra.insert("today_stats".to_string(), stats.clone());
        }
    }
    Ok(build_balance_data(
        daily_quota_limit,
        daily_used,
        balance,
        "USD",
        extra,
    ))
}

fn yescode_balance_extra(combined_data: &Map<String, Value>) -> Map<String, Value> {
    let pay_as_you_go =
        admin_provider_ops_value_as_f64(combined_data.get("pay_as_you_go_balance")).unwrap_or(0.0);
    let subscription =
        admin_provider_ops_value_as_f64(combined_data.get("subscription_balance")).unwrap_or(0.0);
    let plan = combined_data
        .get("subscription_plan")
        .and_then(Value::as_object)
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

    let mut extra = Map::new();
    extra.insert("pay_as_you_go_balance".to_string(), json!(pay_as_you_go));
    extra.insert("daily_limit".to_string(), json!(daily_balance));
    if let Some(limit) = weekly_limit {
        extra.insert("weekly_limit".to_string(), json!(limit));
    }
    extra.insert("weekly_spent".to_string(), json!(weekly_spent));
    if let Some(last_week_reset) = parse_rfc3339_unix_secs(combined_data.get("last_week_reset")) {
        extra.insert(
            "weekly_resets_at".to_string(),
            json!(last_week_reset + 7 * 24 * 3600),
        );
    }
    if let Some(last_daily_add) =
        parse_rfc3339_unix_secs(combined_data.get("last_daily_balance_add"))
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

fn quota_divisor(action_config: &Map<String, Value>) -> f64 {
    admin_provider_ops_value_as_f64(action_config.get("quota_divisor"))
        .filter(|value| *value > 0.0)
        .unwrap_or(500000.0)
}

fn parse_rfc3339_unix_secs(value: Option<&Value>) -> Option<i64> {
    let raw = value?.as_str()?.trim();
    if raw.is_empty() {
        return None;
    }
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|value| value.timestamp())
}

fn value_as_f64(value: Option<&Value>) -> Option<f64> {
    match value {
        Some(Value::Number(number)) => number.as_f64(),
        Some(Value::String(raw)) => raw.trim().parse::<f64>().ok(),
        _ => None,
    }
}

fn parse_subscription(value: &Value) -> Option<Value> {
    let item = value.as_object()?;
    let mut subscription = Map::new();
    subscription.insert(
        "group_name".to_string(),
        item.get("group_name")
            .cloned()
            .unwrap_or_else(|| Value::String(String::new())),
    );
    subscription.insert(
        "status".to_string(),
        item.get("status")
            .cloned()
            .unwrap_or_else(|| Value::String(String::new())),
    );
    for field in [
        "daily_used_usd",
        "daily_limit_usd",
        "weekly_used_usd",
        "weekly_limit_usd",
        "monthly_used_usd",
        "monthly_limit_usd",
        "expires_at",
    ] {
        if let Some(value) = item.get(field).filter(|value| !value.is_null()) {
            subscription.insert(field.to_string(), value.clone());
        }
    }
    Some(Value::Object(subscription))
}

#[cfg(test)]
mod tests {
    use super::{
        attach_balance_checkin_outcome, parse_query_balance_payload, parse_sub2api_balance_payload,
        ProviderOpsCheckinOutcome,
    };
    use serde_json::{json, Map};

    #[test]
    fn anyrouter_single_request_parser_uses_usage_fields() {
        let payload = parse_query_balance_payload(
            "anyrouter",
            &json!({ "quota_divisor": 500000 })
                .as_object()
                .cloned()
                .expect("config"),
            &json!({
                "quota": 2500000,
                "used_quota": 500000
            }),
        )
        .expect("payload should parse");

        assert_eq!(payload["total_available"], json!(5.0));
        assert_eq!(payload["total_used"], json!(1.0));
    }

    #[test]
    fn usage_api_parser_reads_remaining_and_unit() {
        let payload = parse_query_balance_payload(
            "usage_api",
            &json!({ "currency": "USD" })
                .as_object()
                .cloned()
                .expect("config"),
            &json!({
                "remaining": 42.5,
                "balance": 42.5,
                "unit": "USD",
                "isValid": true,
                "planName": "Example Plan"
            }),
        )
        .expect("payload should parse");

        assert_eq!(payload["total_available"], json!(42.5));
        assert_eq!(payload["currency"], json!("USD"));
        assert_eq!(payload["extra"]["plan_name"], json!("Example Plan"));
        assert_eq!(payload["extra"]["is_valid"], json!(true));
    }

    #[test]
    fn usage_api_parser_falls_back_to_nested_quota() {
        let payload = parse_query_balance_payload(
            "usage_api",
            &Map::new(),
            &json!({
                "quota": {
                    "remaining": "12.5",
                    "unit": "CNY"
                }
            }),
        )
        .expect("payload should parse");

        assert_eq!(payload["total_available"], json!(12.5));
        assert_eq!(payload["currency"], json!("CNY"));
    }

    #[test]
    fn done_hub_single_request_parser_reads_wrapped_quota() {
        let payload = parse_query_balance_payload(
            "done_hub",
            &json!({ "quota_divisor": 500000 })
                .as_object()
                .cloned()
                .expect("config"),
            &json!({
                "success": true,
                "data": {
                    "quota": 2_276_139_911_u64,
                    "used_quota": 13860089
                }
            }),
        )
        .expect("payload should parse");

        assert_eq!(payload["total_available"], json!(4552.279822));
        assert_eq!(payload["total_used"], json!(27.720178));
    }

    #[test]
    fn sub2api_parser_sums_balance_and_points() {
        let payload = parse_sub2api_balance_payload(
            &json!({ "currency": "USD" })
                .as_object()
                .cloned()
                .expect("config"),
            &json!({
                "code": 0,
                "data": {
                    "balance": 8.5,
                    "points": 1.5
                }
            }),
            Some(&json!({
                "code": 0,
                "data": {
                    "active_count": 2,
                    "subscriptions": []
                }
            })),
        )
        .expect("payload should parse");

        assert_eq!(payload["total_available"], json!(10.0));
        assert_eq!(payload["extra"]["active_subscriptions"], json!(2));
    }

    #[test]
    fn cubence_parser_reads_wrapped_dashboard_overview() {
        let payload = parse_query_balance_payload(
            "cubence",
            &json!({ "currency": "USD" })
                .as_object()
                .cloned()
                .expect("config"),
            &json!({
                "success": true,
                "data": {
                    "balance": {
                        "normal_balance_dollar": 0.6,
                        "subscription_balance_dollar": 0.0,
                        "charity_balance_dollar": 0.0,
                        "total_balance_dollar": 0.6
                    },
                    "subscription_limits": {
                        "five_hour": {
                            "limit": 10,
                            "used": 1,
                            "remaining": 9,
                            "resets_at": 123
                        },
                        "weekly": {
                            "limit": 20,
                            "used": 2,
                            "remaining": 18,
                            "resets_at": 456
                        }
                    }
                }
            }),
        )
        .expect("payload should parse");

        assert_eq!(payload["total_available"], json!(0.6));
        assert_eq!(payload["extra"]["normal_balance"], json!(0.6));
        assert_eq!(payload["extra"]["five_hour_limit"]["remaining"], json!(9));
        assert_eq!(payload["extra"]["weekly_limit"]["remaining"], json!(18));
    }

    #[test]
    fn attach_balance_checkin_outcome_marks_auth_expired() {
        let mut payload = json!({
            "status": "success",
            "data": { "extra": {} }
        });
        attach_balance_checkin_outcome(
            &mut payload,
            &ProviderOpsCheckinOutcome {
                success: None,
                message: "Cookie 已失效".to_string(),
                cookie_expired: true,
            },
        );

        assert_eq!(payload["status"], json!("auth_expired"));
        assert_eq!(payload["data"]["extra"]["cookie_expired"], json!(true));
    }

    #[test]
    fn attach_balance_checkin_outcome_does_not_copy_upstream_message() {
        let mut payload = json!({
            "status": "success",
            "data": { "extra": {} }
        });
        attach_balance_checkin_outcome(
            &mut payload,
            &ProviderOpsCheckinOutcome {
                success: Some(true),
                message: "authorization=Bearer upstream-secret".to_string(),
                cookie_expired: false,
            },
        );

        assert_eq!(
            payload["data"]["extra"]["checkin_message"],
            json!("签到成功")
        );
        assert!(!payload.to_string().contains("upstream-secret"));
    }

    #[test]
    fn balance_parsers_do_not_return_upstream_error_messages() {
        let config = json!({}).as_object().cloned().expect("config");
        let secret = "authorization=Bearer upstream-secret";

        let generic_error = parse_query_balance_payload(
            "generic_api",
            &config,
            &json!({"success": false, "message": secret}),
        )
        .expect_err("generic API failure should be rejected");
        let sub2api_error =
            parse_sub2api_balance_payload(&config, &json!({"code": 401, "message": secret}), None)
                .expect_err("Sub2API failure should be rejected");

        assert_eq!(generic_error, "业务状态码表示失败");
        assert_eq!(sub2api_error, "查询用户信息失败");
        assert!(!generic_error.contains("upstream-secret"));
        assert!(!sub2api_error.contains("upstream-secret"));
    }
}
