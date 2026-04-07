use super::config::{
    admin_provider_ops_config_object, admin_provider_ops_connector_object,
    admin_provider_ops_decrypted_credentials, admin_provider_ops_uses_python_verify_fallback,
    resolve_admin_provider_ops_base_url,
};
use super::support::{AdminProviderOpsCheckinOutcome, ADMIN_PROVIDER_OPS_ACTION_RUST_ONLY_MESSAGE};
use super::verify::{admin_provider_ops_value_as_f64, admin_provider_ops_verify_headers};
use crate::AppState;
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogProvider,
};
use serde_json::json;

pub(super) fn admin_provider_ops_is_valid_action_type(action_type: &str) -> bool {
    matches!(
        action_type,
        "query_balance"
            | "checkin"
            | "claim_quota"
            | "refresh_token"
            | "get_usage"
            | "get_models"
            | "custom"
    )
}

fn admin_provider_ops_action_response(
    status: &str,
    action_type: &str,
    data: serde_json::Value,
    message: Option<String>,
    response_time_ms: Option<u64>,
    cache_ttl_seconds: u64,
) -> serde_json::Value {
    json!({
        "status": status,
        "action_type": action_type,
        "data": data,
        "message": message,
        "executed_at": chrono::Utc::now()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "response_time_ms": response_time_ms,
        "cache_ttl_seconds": cache_ttl_seconds,
    })
}

fn admin_provider_ops_action_error(
    status: &str,
    action_type: &str,
    message: impl Into<String>,
    response_time_ms: Option<u64>,
) -> serde_json::Value {
    admin_provider_ops_action_response(
        status,
        action_type,
        serde_json::Value::Null,
        Some(message.into()),
        response_time_ms,
        0,
    )
}

fn admin_provider_ops_action_not_configured(
    action_type: &str,
    message: impl Into<String>,
) -> serde_json::Value {
    admin_provider_ops_action_error("not_configured", action_type, message, None)
}

fn admin_provider_ops_action_not_supported(
    action_type: &str,
    message: impl Into<String>,
) -> serde_json::Value {
    admin_provider_ops_action_error("not_supported", action_type, message, None)
}

fn admin_provider_ops_balance_data(
    total_granted: Option<f64>,
    total_used: Option<f64>,
    total_available: Option<f64>,
    currency: &str,
    extra: serde_json::Map<String, serde_json::Value>,
) -> serde_json::Value {
    json!({
        "total_granted": total_granted,
        "total_used": total_used,
        "total_available": total_available,
        "expires_at": serde_json::Value::Null,
        "currency": currency,
        "extra": extra,
    })
}

fn admin_provider_ops_checkin_data(
    reward: Option<f64>,
    streak_days: Option<i64>,
    next_reward: Option<f64>,
    message: Option<String>,
    extra: serde_json::Map<String, serde_json::Value>,
) -> serde_json::Value {
    json!({
        "reward": reward,
        "streak_days": streak_days,
        "next_reward": next_reward,
        "message": message,
        "extra": extra,
    })
}

fn admin_provider_ops_action_config_object<'a>(
    provider_ops_config: &'a serde_json::Map<String, serde_json::Value>,
    action_type: &str,
) -> Option<&'a serde_json::Map<String, serde_json::Value>> {
    provider_ops_config
        .get("actions")
        .and_then(serde_json::Value::as_object)
        .and_then(|actions| actions.get(action_type))
        .and_then(serde_json::Value::as_object)
        .and_then(|action| action.get("config"))
        .and_then(serde_json::Value::as_object)
}

fn admin_provider_ops_default_action_config(
    architecture_id: &str,
    action_type: &str,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let value = match (architecture_id, action_type) {
        ("generic_api", "query_balance") => {
            json!({ "endpoint": "/api/user/balance", "method": "GET" })
        }
        ("generic_api", "checkin") => {
            json!({ "endpoint": "/api/user/checkin", "method": "POST" })
        }
        ("new_api", "query_balance") => json!({
            "endpoint": "/api/user/self",
            "method": "GET",
            "quota_divisor": 500000,
            "checkin_endpoint": "/api/user/checkin",
            "currency": "USD",
        }),
        ("new_api", "checkin") => json!({ "endpoint": "/api/user/checkin", "method": "POST" }),
        ("cubence", "query_balance") => {
            json!({ "endpoint": "/api/v1/dashboard/overview", "method": "GET", "currency": "USD" })
        }
        ("yescode", "query_balance") => {
            json!({ "endpoint": "/api/v1/user/balance", "method": "GET", "currency": "USD" })
        }
        ("nekocode", "query_balance") => {
            json!({ "endpoint": "/api/usage/summary", "method": "GET", "currency": "USD" })
        }
        _ => return None,
    };
    value.as_object().cloned()
}

fn admin_provider_ops_json_object_map(
    value: serde_json::Value,
) -> serde_json::Map<String, serde_json::Value> {
    value.as_object().cloned().unwrap_or_default()
}

fn admin_provider_ops_resolved_action_config(
    architecture_id: &str,
    provider_ops_config: &serde_json::Map<String, serde_json::Value>,
    action_type: &str,
    request_config: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let mut resolved =
        admin_provider_ops_default_action_config(architecture_id, action_type).unwrap_or_default();
    if let Some(saved) = admin_provider_ops_action_config_object(provider_ops_config, action_type) {
        for (key, value) in saved {
            resolved.insert(key.clone(), value.clone());
        }
    }
    if let Some(overrides) = request_config {
        for (key, value) in overrides {
            resolved.insert(key.clone(), value.clone());
        }
    }
    (!resolved.is_empty()).then_some(resolved)
}

fn admin_provider_ops_request_url(
    base_url: &str,
    action_config: &serde_json::Map<String, serde_json::Value>,
    default_endpoint: &str,
) -> String {
    let endpoint = action_config
        .get("endpoint")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(default_endpoint);
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        endpoint.to_string()
    } else {
        format!("{}{}", base_url.trim_end_matches('/'), endpoint)
    }
}

fn admin_provider_ops_request_method(
    action_config: &serde_json::Map<String, serde_json::Value>,
    default_method: &str,
) -> reqwest::Method {
    action_config
        .get("method")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| reqwest::Method::from_bytes(value.trim().as_bytes()).ok())
        .unwrap_or_else(|| {
            reqwest::Method::from_bytes(default_method.as_bytes()).unwrap_or(reqwest::Method::GET)
        })
}

fn admin_provider_ops_message_contains_any(message: &str, indicators: &[&str]) -> bool {
    let normalized = message.trim().to_ascii_lowercase();
    indicators
        .iter()
        .any(|indicator| normalized.contains(&indicator.to_ascii_lowercase()))
}

fn admin_provider_ops_checkin_already_done(message: &str) -> bool {
    admin_provider_ops_message_contains_any(
        message,
        &["already", "已签到", "已经签到", "今日已签", "重复签到"],
    )
}

fn admin_provider_ops_checkin_auth_failure(message: &str) -> bool {
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

fn admin_provider_ops_checkin_payload(
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

fn admin_provider_ops_parse_rfc3339_unix_secs(value: Option<&serde_json::Value>) -> Option<i64> {
    let raw = value?.as_str()?.trim();
    if raw.is_empty() {
        return None;
    }
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|value| value.timestamp())
}

fn admin_provider_ops_is_cookie_auth_architecture(architecture_id: &str) -> bool {
    matches!(architecture_id, "cubence" | "yescode" | "nekocode")
}

fn admin_provider_ops_should_use_rust_only_action_stub(
    architecture_id: &str,
    config: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    !matches!(
        architecture_id,
        "generic_api" | "new_api" | "cubence" | "yescode" | "nekocode"
    ) || admin_provider_ops_uses_python_verify_fallback(architecture_id, config)
}

async fn admin_provider_ops_probe_new_api_checkin(
    state: &AppState,
    base_url: &str,
    action_config: &serde_json::Map<String, serde_json::Value>,
    headers: &reqwest::header::HeaderMap,
    has_cookie: bool,
) -> Option<AdminProviderOpsCheckinOutcome> {
    let endpoint = action_config
        .get("checkin_endpoint")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("/api/user/checkin");
    let url = admin_provider_ops_request_url(
        base_url,
        &admin_provider_ops_json_object_map(json!({ "endpoint": endpoint })),
        endpoint,
    );
    let response = match state
        .client
        .request(reqwest::Method::POST, url)
        .headers(headers.clone())
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => return None,
    };

    if response.status() == http::StatusCode::NOT_FOUND {
        return None;
    }
    if matches!(
        response.status(),
        http::StatusCode::UNAUTHORIZED | http::StatusCode::FORBIDDEN
    ) {
        return has_cookie.then(|| AdminProviderOpsCheckinOutcome {
            success: None,
            message: "Cookie 已失效".to_string(),
            cookie_expired: true,
        });
    }

    let response_json = match response.bytes().await {
        Ok(bytes) => {
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or_else(|_| json!({}))
        }
        Err(_) => json!({}),
    };
    let message = response_json
        .get("message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if response_json
        .get("success")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        return Some(AdminProviderOpsCheckinOutcome {
            success: Some(true),
            message: if message.is_empty() {
                "签到成功".to_string()
            } else {
                message
            },
            cookie_expired: false,
        });
    }
    if admin_provider_ops_checkin_already_done(&message) {
        return Some(AdminProviderOpsCheckinOutcome {
            success: None,
            message: if message.is_empty() {
                "今日已签到".to_string()
            } else {
                message
            },
            cookie_expired: false,
        });
    }
    if admin_provider_ops_checkin_auth_failure(&message) {
        return has_cookie.then(|| AdminProviderOpsCheckinOutcome {
            success: None,
            message: if message.is_empty() {
                "Cookie 已失效".to_string()
            } else {
                message
            },
            cookie_expired: true,
        });
    }
    Some(AdminProviderOpsCheckinOutcome {
        success: Some(false),
        message: if message.is_empty() {
            "签到失败".to_string()
        } else {
            message
        },
        cookie_expired: false,
    })
}

async fn admin_provider_ops_run_checkin_action(
    state: &AppState,
    base_url: &str,
    architecture_id: &str,
    action_config: &serde_json::Map<String, serde_json::Value>,
    headers: &reqwest::header::HeaderMap,
    has_cookie: bool,
) -> serde_json::Value {
    let start = std::time::Instant::now();
    if !matches!(architecture_id, "generic_api" | "new_api") {
        return admin_provider_ops_action_not_supported(
            "checkin",
            ADMIN_PROVIDER_OPS_ACTION_RUST_ONLY_MESSAGE,
        );
    }

    let url = admin_provider_ops_request_url(base_url, action_config, "/api/user/checkin");
    let method = admin_provider_ops_request_method(action_config, "POST");
    let response = match state
        .client
        .request(method, url)
        .headers(headers.clone())
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) if err.is_timeout() => {
            return admin_provider_ops_action_error("network_error", "checkin", "请求超时", None);
        }
        Err(err) => {
            return admin_provider_ops_action_error(
                "network_error",
                "checkin",
                format!("网络错误: {err}"),
                None,
            );
        }
    };
    let response_time_ms = Some(start.elapsed().as_millis() as u64);
    let status = response.status();
    let response_json = match response.bytes().await {
        Ok(bytes) => match serde_json::from_slice::<serde_json::Value>(&bytes) {
            Ok(value) => value,
            Err(_) => {
                return admin_provider_ops_action_error(
                    "parse_error",
                    "checkin",
                    "响应不是有效的 JSON",
                    response_time_ms,
                );
            }
        },
        Err(err) => {
            return admin_provider_ops_action_error(
                "network_error",
                "checkin",
                format!("网络错误: {err}"),
                response_time_ms,
            );
        }
    };

    if status == http::StatusCode::NOT_FOUND {
        return admin_provider_ops_action_error(
            "not_supported",
            "checkin",
            "功能未开放",
            response_time_ms,
        );
    }
    if status == http::StatusCode::TOO_MANY_REQUESTS {
        return admin_provider_ops_action_error(
            "rate_limited",
            "checkin",
            "请求频率限制",
            response_time_ms,
        );
    }
    if status == http::StatusCode::UNAUTHORIZED {
        return admin_provider_ops_action_error(
            if has_cookie {
                "auth_expired"
            } else {
                "auth_failed"
            },
            "checkin",
            if has_cookie {
                "Cookie 已失效，请重新配置"
            } else {
                "认证失败"
            },
            response_time_ms,
        );
    }
    if status == http::StatusCode::FORBIDDEN {
        return admin_provider_ops_action_error(
            if has_cookie {
                "auth_expired"
            } else {
                "auth_failed"
            },
            "checkin",
            if has_cookie {
                "Cookie 已失效或无权限"
            } else {
                "无权限访问"
            },
            response_time_ms,
        );
    }
    if status != http::StatusCode::OK {
        return admin_provider_ops_action_error(
            "unknown_error",
            "checkin",
            format!(
                "HTTP {}: {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("Unknown")
            ),
            response_time_ms,
        );
    }

    let message = response_json
        .get("message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if response_json
        .get("success")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        return admin_provider_ops_action_response(
            "success",
            "checkin",
            admin_provider_ops_checkin_payload(&response_json, Some(message)),
            None,
            response_time_ms,
            3600,
        );
    }
    if admin_provider_ops_checkin_already_done(&message) {
        return admin_provider_ops_action_response(
            "already_done",
            "checkin",
            admin_provider_ops_checkin_payload(&response_json, Some(message)),
            None,
            response_time_ms,
            3600,
        );
    }
    if admin_provider_ops_checkin_auth_failure(&message) {
        return admin_provider_ops_action_error(
            if has_cookie {
                "auth_expired"
            } else {
                "auth_failed"
            },
            "checkin",
            if message.is_empty() {
                if has_cookie {
                    "Cookie 已失效"
                } else {
                    "认证失败"
                }
            } else {
                message.as_str()
            },
            response_time_ms,
        );
    }
    admin_provider_ops_action_error(
        "unknown_error",
        "checkin",
        if message.is_empty() {
            "签到失败"
        } else {
            message.as_str()
        },
        response_time_ms,
    )
}

fn admin_provider_ops_new_api_balance_payload(
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
    let quota_divisor = admin_provider_ops_value_as_f64(action_config.get("quota_divisor"))
        .filter(|value| *value > 0.0)
        .unwrap_or(500000.0);
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

fn admin_provider_ops_cubence_balance_payload(
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

fn admin_provider_ops_yescode_balance_extra(
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

async fn admin_provider_ops_yescode_balance_payload(
    state: &AppState,
    base_url: &str,
    headers: &reqwest::header::HeaderMap,
    action_config: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Value {
    let start = std::time::Instant::now();
    let balance_url = format!("{}/api/v1/user/balance", base_url.trim_end_matches('/'));
    let profile_url = format!("{}/api/v1/auth/profile", base_url.trim_end_matches('/'));
    let balance_future = state
        .client
        .request(reqwest::Method::GET, balance_url)
        .headers(headers.clone())
        .send();
    let profile_future = state
        .client
        .request(reqwest::Method::GET, profile_url)
        .headers(headers.clone())
        .send();
    let (balance_result, profile_result) = tokio::join!(balance_future, profile_future);
    let response_time_ms = Some(start.elapsed().as_millis() as u64);

    let mut combined = serde_json::Map::new();
    let mut has_any = false;

    if let Ok(balance_response) = balance_result {
        if balance_response.status() == http::StatusCode::OK {
            if let Ok(bytes) = balance_response.bytes().await {
                if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                    if let Some(object) = value.as_object() {
                        has_any = true;
                        combined.insert(
                            "_balance_data".to_string(),
                            serde_json::Value::Object(object.clone()),
                        );
                        combined.insert(
                            "pay_as_you_go_balance".to_string(),
                            object
                                .get("pay_as_you_go_balance")
                                .cloned()
                                .unwrap_or_else(|| json!(0)),
                        );
                        combined.insert(
                            "subscription_balance".to_string(),
                            object
                                .get("subscription_balance")
                                .cloned()
                                .unwrap_or_else(|| json!(0)),
                        );
                        if let Some(limit) = object.get("weekly_limit") {
                            combined.insert("weekly_limit".to_string(), limit.clone());
                        }
                        combined.insert(
                            "weekly_spent_balance".to_string(),
                            object
                                .get("weekly_spent_balance")
                                .cloned()
                                .unwrap_or_else(|| json!(0)),
                        );
                    }
                }
            }
        }
    }

    if let Ok(profile_response) = profile_result {
        if profile_response.status() == http::StatusCode::OK {
            if let Ok(bytes) = profile_response.bytes().await {
                if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                    if let Some(object) = value.as_object() {
                        has_any = true;
                        combined.insert(
                            "_profile_data".to_string(),
                            serde_json::Value::Object(object.clone()),
                        );
                        for key in [
                            "username",
                            "email",
                            "last_week_reset",
                            "last_daily_balance_add",
                            "subscription_plan",
                        ] {
                            if let Some(value) = object.get(key) {
                                combined.insert(key.to_string(), value.clone());
                            }
                        }
                        combined
                            .entry("pay_as_you_go_balance".to_string())
                            .or_insert_with(|| {
                                object
                                    .get("pay_as_you_go_balance")
                                    .cloned()
                                    .unwrap_or_else(|| json!(0))
                            });
                        combined
                            .entry("subscription_balance".to_string())
                            .or_insert_with(|| {
                                object
                                    .get("subscription_balance")
                                    .cloned()
                                    .unwrap_or_else(|| json!(0))
                            });
                        combined
                            .entry("weekly_spent_balance".to_string())
                            .or_insert_with(|| {
                                object
                                    .get("current_week_spend")
                                    .cloned()
                                    .unwrap_or_else(|| json!(0))
                            });
                        if !combined.contains_key("weekly_limit") {
                            if let Some(limit) = object
                                .get("subscription_plan")
                                .and_then(serde_json::Value::as_object)
                                .and_then(|plan| plan.get("weekly_limit"))
                            {
                                combined.insert("weekly_limit".to_string(), limit.clone());
                            }
                        }
                    }
                }
            }
        }
    }

    if !has_any {
        return admin_provider_ops_action_error(
            "auth_failed",
            "query_balance",
            "Cookie 已失效，请重新配置",
            response_time_ms,
        );
    }

    let mut extra = admin_provider_ops_yescode_balance_extra(&combined);
    let total_available = admin_provider_ops_value_as_f64(extra.get("_total_available"));
    extra.remove("_subscription_available");
    extra.remove("_total_available");
    admin_provider_ops_action_response(
        "success",
        "query_balance",
        admin_provider_ops_balance_data(
            None,
            None,
            total_available,
            action_config
                .get("currency")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("USD"),
            extra,
        ),
        None,
        response_time_ms,
        86400,
    )
}

fn admin_provider_ops_nekocode_balance_payload(
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

fn admin_provider_ops_attach_balance_checkin_outcome(
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

async fn admin_provider_ops_run_query_balance_action(
    state: &AppState,
    base_url: &str,
    architecture_id: &str,
    connector_config: &serde_json::Map<String, serde_json::Value>,
    action_config: &serde_json::Map<String, serde_json::Value>,
    headers: &reqwest::header::HeaderMap,
    credentials: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Value {
    if architecture_id == "yescode" {
        return admin_provider_ops_yescode_balance_payload(state, base_url, headers, action_config)
            .await;
    }

    let mut balance_checkin = None;
    if matches!(architecture_id, "generic_api" | "new_api") {
        let has_cookie = credentials
            .get("cookie")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| !value.trim().is_empty());
        balance_checkin = admin_provider_ops_probe_new_api_checkin(
            state,
            base_url,
            action_config,
            headers,
            has_cookie,
        )
        .await;
    }

    let start = std::time::Instant::now();
    let url = admin_provider_ops_request_url(base_url, action_config, "/api/user/balance");
    let method = admin_provider_ops_request_method(action_config, "GET");
    let response = match state
        .client
        .request(method, url)
        .headers(headers.clone())
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) if err.is_timeout() => {
            return admin_provider_ops_action_error(
                "network_error",
                "query_balance",
                "请求超时",
                None,
            );
        }
        Err(err) => {
            return admin_provider_ops_action_error(
                "network_error",
                "query_balance",
                format!("网络错误: {err}"),
                None,
            );
        }
    };
    let response_time_ms = Some(start.elapsed().as_millis() as u64);
    let status = response.status();
    let response_json = match response.bytes().await {
        Ok(bytes) => match serde_json::from_slice::<serde_json::Value>(&bytes) {
            Ok(value) => value,
            Err(_) => {
                return admin_provider_ops_action_error(
                    "parse_error",
                    "query_balance",
                    "响应不是有效的 JSON",
                    response_time_ms,
                );
            }
        },
        Err(err) => {
            return admin_provider_ops_action_error(
                "network_error",
                "query_balance",
                format!("网络错误: {err}"),
                response_time_ms,
            );
        }
    };

    if status != http::StatusCode::OK {
        let cookie_auth = admin_provider_ops_is_cookie_auth_architecture(architecture_id);
        let payload = match status {
            http::StatusCode::UNAUTHORIZED => admin_provider_ops_action_error(
                "auth_failed",
                "query_balance",
                if cookie_auth {
                    "Cookie 已失效，请重新配置"
                } else {
                    "认证失败"
                },
                response_time_ms,
            ),
            http::StatusCode::FORBIDDEN => admin_provider_ops_action_error(
                "auth_failed",
                "query_balance",
                if cookie_auth {
                    "Cookie 已失效或无权限"
                } else {
                    "无权限访问"
                },
                response_time_ms,
            ),
            http::StatusCode::NOT_FOUND => admin_provider_ops_action_error(
                "not_supported",
                "query_balance",
                "功能未开放",
                response_time_ms,
            ),
            http::StatusCode::TOO_MANY_REQUESTS => admin_provider_ops_action_error(
                "rate_limited",
                "query_balance",
                "请求频率限制",
                response_time_ms,
            ),
            _ => admin_provider_ops_action_error(
                "unknown_error",
                "query_balance",
                format!(
                    "HTTP {}: {}",
                    status.as_u16(),
                    status.canonical_reason().unwrap_or("Unknown")
                ),
                response_time_ms,
            ),
        };
        return payload;
    }

    let data = match architecture_id {
        "generic_api" | "new_api" => {
            match admin_provider_ops_new_api_balance_payload(action_config, &response_json) {
                Ok(data) => data,
                Err(message) => {
                    return admin_provider_ops_action_error(
                        "unknown_error",
                        "query_balance",
                        message,
                        response_time_ms,
                    );
                }
            }
        }
        "cubence" => {
            match admin_provider_ops_cubence_balance_payload(action_config, &response_json) {
                Ok(data) => data,
                Err(message) => {
                    return admin_provider_ops_action_error(
                        "parse_error",
                        "query_balance",
                        message,
                        response_time_ms,
                    );
                }
            }
        }
        "nekocode" => match admin_provider_ops_nekocode_balance_payload(&response_json) {
            Ok(data) => data,
            Err(message) => {
                return admin_provider_ops_action_error(
                    "parse_error",
                    "query_balance",
                    message,
                    response_time_ms,
                );
            }
        },
        _ => {
            return admin_provider_ops_action_not_supported(
                "query_balance",
                ADMIN_PROVIDER_OPS_ACTION_RUST_ONLY_MESSAGE,
            );
        }
    };

    let mut payload = admin_provider_ops_action_response(
        "success",
        "query_balance",
        data,
        None,
        response_time_ms,
        86400,
    );
    if let Some(outcome) = balance_checkin.as_ref() {
        admin_provider_ops_attach_balance_checkin_outcome(&mut payload, outcome);
    }
    let _ = connector_config;
    payload
}

pub(crate) async fn admin_provider_ops_local_action_response(
    state: &AppState,
    _provider_id: &str,
    provider: Option<&StoredProviderCatalogProvider>,
    endpoints: &[StoredProviderCatalogEndpoint],
    action_type: &str,
    request_config: Option<&serde_json::Map<String, serde_json::Value>>,
) -> serde_json::Value {
    let Some(provider) = provider else {
        return admin_provider_ops_action_not_configured(action_type, "未配置操作设置");
    };
    let Some(provider_ops_config) = admin_provider_ops_config_object(provider) else {
        return admin_provider_ops_action_not_configured(action_type, "未配置操作设置");
    };
    let architecture_id = provider_ops_config
        .get("architecture_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("generic_api");
    let connector_config = admin_provider_ops_connector_object(provider_ops_config)
        .and_then(|connector| connector.get("config"))
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    if admin_provider_ops_should_use_rust_only_action_stub(architecture_id, &connector_config) {
        return admin_provider_ops_action_not_supported(
            action_type,
            ADMIN_PROVIDER_OPS_ACTION_RUST_ONLY_MESSAGE,
        );
    }

    let Some(base_url) =
        resolve_admin_provider_ops_base_url(provider, endpoints, Some(provider_ops_config))
    else {
        return admin_provider_ops_action_not_configured(action_type, "Provider 未配置 base_url");
    };
    let credentials = admin_provider_ops_decrypted_credentials(
        state,
        admin_provider_ops_config_object(provider)
            .and_then(admin_provider_ops_connector_object)
            .and_then(|connector| connector.get("credentials")),
    );
    let headers =
        match admin_provider_ops_verify_headers(architecture_id, &connector_config, &credentials) {
            Ok(headers) => headers,
            Err(message) => return admin_provider_ops_action_not_configured(action_type, message),
        };
    let Some(action_config) = admin_provider_ops_resolved_action_config(
        architecture_id,
        provider_ops_config,
        action_type,
        request_config,
    ) else {
        return admin_provider_ops_action_not_supported(
            action_type,
            ADMIN_PROVIDER_OPS_ACTION_RUST_ONLY_MESSAGE,
        );
    };

    match action_type {
        "query_balance" => {
            admin_provider_ops_run_query_balance_action(
                state,
                &base_url,
                architecture_id,
                &connector_config,
                &action_config,
                &headers,
                &credentials,
            )
            .await
        }
        "checkin" => {
            let has_cookie = credentials
                .get("cookie")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|value| !value.trim().is_empty());
            admin_provider_ops_run_checkin_action(
                state,
                &base_url,
                architecture_id,
                &action_config,
                &headers,
                has_cookie,
            )
            .await
        }
        _ => admin_provider_ops_action_not_supported(
            action_type,
            ADMIN_PROVIDER_OPS_ACTION_RUST_ONLY_MESSAGE,
        ),
    }
}
