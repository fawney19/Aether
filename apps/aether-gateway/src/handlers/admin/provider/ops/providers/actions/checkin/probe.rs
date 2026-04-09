use super::super::super::support::AdminProviderOpsCheckinOutcome;
use super::super::support::{admin_provider_ops_json_object_map, admin_provider_ops_request_url};
use super::shared::{
    admin_provider_ops_checkin_already_done, admin_provider_ops_checkin_auth_failure,
};
use crate::handlers::admin::request::AdminAppState;
use serde_json::json;

pub(in super::super) async fn admin_provider_ops_probe_new_api_checkin(
    state: &AdminAppState<'_>,
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
        .http_client()
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
