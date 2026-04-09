use super::super::super::verify::admin_provider_ops_value_as_f64;
use super::super::responses::{
    admin_provider_ops_action_error, admin_provider_ops_action_response,
};
use super::super::support::admin_provider_ops_balance_data;
use super::parsers::admin_provider_ops_yescode_balance_extra;
use crate::handlers::admin::request::AdminAppState;
use serde_json::json;

pub(super) async fn admin_provider_ops_yescode_balance_payload(
    state: &AdminAppState<'_>,
    base_url: &str,
    headers: &reqwest::header::HeaderMap,
    action_config: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Value {
    let start = std::time::Instant::now();
    let balance_url = format!("{}/api/v1/user/balance", base_url.trim_end_matches('/'));
    let profile_url = format!("{}/api/v1/auth/profile", base_url.trim_end_matches('/'));
    let balance_future = state
        .http_client()
        .request(reqwest::Method::GET, balance_url)
        .headers(headers.clone())
        .send();
    let profile_future = state
        .http_client()
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
