use super::super::super::support::AdminProviderOpsCheckinOutcome;
use super::super::super::verify::{
    admin_provider_ops_execute_json_request, AdminProviderOpsExecuteJsonError,
};
use super::super::support::{admin_provider_ops_json_object_map, admin_provider_ops_request_url};
use super::shared::{
    admin_provider_ops_checkin_already_done, admin_provider_ops_checkin_auth_failure,
};
use crate::handlers::admin::request::AdminAppState;
use aether_contracts::ProxySnapshot;
use serde_json::json;

pub(in super::super) async fn admin_provider_ops_probe_new_api_checkin(
    state: &AdminAppState<'_>,
    base_url: &str,
    action_config: &serde_json::Map<String, serde_json::Value>,
    headers: &reqwest::header::HeaderMap,
    has_cookie: bool,
    proxy_snapshot: Option<&ProxySnapshot>,
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
    let (status, response_json) = match admin_provider_ops_execute_json_request(
        state,
        "provider-ops-action:probe_checkin",
        reqwest::Method::POST,
        &url,
        headers,
        None,
        proxy_snapshot,
    )
    .await
    {
        Ok(result) => result,
        Err(AdminProviderOpsExecuteJsonError::InvalidJson(_))
        | Err(AdminProviderOpsExecuteJsonError::Transport(_)) => return None,
    };

    if status == http::StatusCode::NOT_FOUND {
        return None;
    }
    if matches!(
        status,
        http::StatusCode::UNAUTHORIZED | http::StatusCode::FORBIDDEN
    ) {
        return has_cookie.then(|| AdminProviderOpsCheckinOutcome {
            success: None,
            message: "Cookie 已失效".to_string(),
            cookie_expired: true,
        });
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
