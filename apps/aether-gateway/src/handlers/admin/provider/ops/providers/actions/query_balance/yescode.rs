use super::super::super::verify::{
    admin_provider_ops_execute_json_request, AdminProviderOpsExecuteJsonError,
};
use super::super::responses::{
    admin_provider_ops_action_error, admin_provider_ops_action_response,
};
use crate::handlers::admin::request::AdminAppState;
use aether_admin::provider::ops::parse_yescode_combined_balance_payload;
use aether_contracts::ProxySnapshot;
use serde_json::json;

pub(super) async fn admin_provider_ops_yescode_balance_payload(
    state: &AdminAppState<'_>,
    base_url: &str,
    headers: &reqwest::header::HeaderMap,
    action_config: &serde_json::Map<String, serde_json::Value>,
    proxy_snapshot: Option<&ProxySnapshot>,
) -> serde_json::Value {
    let start = std::time::Instant::now();
    let balance_url = format!("{}/api/v1/user/balance", base_url.trim_end_matches('/'));
    let profile_url = format!("{}/api/v1/auth/profile", base_url.trim_end_matches('/'));
    let (balance_result, profile_result) = tokio::join!(
        admin_provider_ops_execute_json_request(
            state,
            "provider-ops-action:yescode:balance",
            reqwest::Method::GET,
            &balance_url,
            headers,
            None,
            proxy_snapshot,
        ),
        admin_provider_ops_execute_json_request(
            state,
            "provider-ops-action:yescode:profile",
            reqwest::Method::GET,
            &profile_url,
            headers,
            None,
            proxy_snapshot,
        )
    );
    let balance_result = balance_result.map_err(|err| match err {
        AdminProviderOpsExecuteJsonError::InvalidJson(message)
        | AdminProviderOpsExecuteJsonError::Transport(message) => message,
    });
    let profile_result = profile_result.map_err(|err| match err {
        AdminProviderOpsExecuteJsonError::InvalidJson(message)
        | AdminProviderOpsExecuteJsonError::Transport(message) => message,
    });
    let response_time_ms = Some(start.elapsed().as_millis() as u64);

    let mut combined = serde_json::Map::new();
    let mut has_any = false;

    if let Ok((status, value)) = balance_result {
        if status == http::StatusCode::OK {
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

    if let Ok((status, value)) = profile_result {
        if status == http::StatusCode::OK {
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

    if !has_any {
        return admin_provider_ops_action_error(
            "auth_failed",
            "query_balance",
            "Cookie 已失效，请重新配置",
            response_time_ms,
        );
    }

    admin_provider_ops_action_response(
        "success",
        "query_balance",
        parse_yescode_combined_balance_payload(action_config, &combined),
        None,
        response_time_ms,
        86400,
    )
}
