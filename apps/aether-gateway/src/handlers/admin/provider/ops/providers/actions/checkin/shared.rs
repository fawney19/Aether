use super::super::super::verify::admin_provider_ops_value_as_f64;
use super::super::support::admin_provider_ops_checkin_data;

fn admin_provider_ops_message_contains_any(message: &str, indicators: &[&str]) -> bool {
    let normalized = message.trim().to_ascii_lowercase();
    indicators
        .iter()
        .any(|indicator| normalized.contains(&indicator.to_ascii_lowercase()))
}

pub(super) fn admin_provider_ops_checkin_already_done(message: &str) -> bool {
    admin_provider_ops_message_contains_any(
        message,
        &["already", "已签到", "已经签到", "今日已签", "重复签到"],
    )
}

pub(super) fn admin_provider_ops_checkin_auth_failure(message: &str) -> bool {
    admin_provider_ops_message_contains_any(
        message,
        &[
            "未登录",
            "请登录",
            "login",
            "unauthorized",
            "无权限",
            "权限不足",
            "turnstile",
            "captcha",
            "验证码",
        ],
    )
}

pub(super) fn admin_provider_ops_checkin_payload(
    response_json: &serde_json::Value,
    fallback_message: Option<String>,
) -> serde_json::Value {
    let details = response_json
        .get("data")
        .and_then(serde_json::Value::as_object)
        .or_else(|| response_json.as_object());
    let reward = details.and_then(|value| {
        admin_provider_ops_value_as_f64(
            value
                .get("reward")
                .or_else(|| value.get("quota"))
                .or_else(|| value.get("amount")),
        )
    });
    let streak_days = details
        .and_then(|value| value.get("streak_days").or_else(|| value.get("streak")))
        .and_then(serde_json::Value::as_i64);
    let next_reward = details.and_then(|value| {
        admin_provider_ops_value_as_f64(value.get("next_reward").or_else(|| value.get("next")))
    });
    let message = fallback_message.or_else(|| {
        response_json
            .get("message")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
    });
    let mut extra = serde_json::Map::new();
    if let Some(details) = details {
        for (key, value) in details {
            if matches!(
                key.as_str(),
                "reward"
                    | "quota"
                    | "amount"
                    | "streak_days"
                    | "streak"
                    | "next_reward"
                    | "next"
                    | "message"
            ) {
                continue;
            }
            extra.insert(key.clone(), value.clone());
        }
    }
    admin_provider_ops_checkin_data(reward, streak_days, next_reward, message, extra)
}
