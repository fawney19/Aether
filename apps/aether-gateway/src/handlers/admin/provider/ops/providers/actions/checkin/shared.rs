use super::super::support::admin_provider_ops_checkin_data;
use aether_admin::provider::ops::admin_provider_ops_value_as_f64;

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
    message: Option<String>,
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
    admin_provider_ops_checkin_data(
        reward,
        streak_days,
        next_reward,
        message,
        serde_json::Map::new(),
    )
}

#[cfg(test)]
mod tests {
    use super::admin_provider_ops_checkin_payload;
    use serde_json::json;

    #[test]
    fn checkin_payload_keeps_metrics_without_copying_upstream_secrets() {
        let payload = admin_provider_ops_checkin_payload(
            &json!({
                "success": true,
                "message": "authorization=Bearer upstream-secret",
                "data": {
                    "reward": 1.5,
                    "streak_days": 3,
                    "next_reward": 2.0,
                    "api_key": "secret-api-key",
                    "profile": {"access_token": "secret-token"}
                }
            }),
            Some("签到成功".to_string()),
        );

        assert_eq!(payload["reward"], json!(1.5));
        assert_eq!(payload["streak_days"], json!(3));
        assert_eq!(payload["next_reward"], json!(2.0));
        assert_eq!(payload["message"], json!("签到成功"));
        assert_eq!(payload["extra"], json!({}));
        let serialized = payload.to_string();
        assert!(!serialized.contains("upstream-secret"));
        assert!(!serialized.contains("secret-api-key"));
        assert!(!serialized.contains("secret-token"));
    }
}
