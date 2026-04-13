use super::super::super::support::ADMIN_PROVIDER_OPS_ACTION_RUST_ONLY_MESSAGE;
use super::super::super::verify::{
    admin_provider_ops_execute_json_request, AdminProviderOpsExecuteJsonError,
};
use super::super::responses::{
    admin_provider_ops_action_error, admin_provider_ops_action_not_supported,
    admin_provider_ops_action_response,
};
use super::super::support::{admin_provider_ops_request_method, admin_provider_ops_request_url};
use super::shared::{
    admin_provider_ops_checkin_already_done, admin_provider_ops_checkin_auth_failure,
    admin_provider_ops_checkin_payload,
};
use crate::handlers::admin::request::AdminAppState;
use aether_admin::provider::ops::{ProviderOpsArchitectureSpec, ProviderOpsCheckinMode};
use aether_contracts::ProxySnapshot;

pub(in super::super) async fn admin_provider_ops_run_checkin_action(
    state: &AdminAppState<'_>,
    base_url: &str,
    architecture: &ProviderOpsArchitectureSpec,
    action_config: &serde_json::Map<String, serde_json::Value>,
    headers: &reqwest::header::HeaderMap,
    has_cookie: bool,
    proxy_snapshot: Option<&ProxySnapshot>,
) -> serde_json::Value {
    let start = std::time::Instant::now();
    if architecture.checkin_mode != ProviderOpsCheckinMode::NewApiCompatible {
        return admin_provider_ops_action_not_supported(
            "checkin",
            ADMIN_PROVIDER_OPS_ACTION_RUST_ONLY_MESSAGE,
        );
    }

    let url = admin_provider_ops_request_url(base_url, action_config, "/api/user/checkin");
    let method = admin_provider_ops_request_method(action_config, "POST");
    let (status, response_json) = match admin_provider_ops_execute_json_request(
        state,
        &format!(
            "provider-ops-action:{}:checkin",
            architecture.architecture_id
        ),
        method,
        &url,
        headers,
        None,
        proxy_snapshot,
    )
    .await
    {
        Ok(result) => result,
        Err(AdminProviderOpsExecuteJsonError::InvalidJson(_)) => {
            return admin_provider_ops_action_error(
                "parse_error",
                "checkin",
                "响应不是有效的 JSON",
                Some(start.elapsed().as_millis() as u64),
            );
        }
        Err(AdminProviderOpsExecuteJsonError::Transport(err)) => {
            return admin_provider_ops_action_error(
                "network_error",
                "checkin",
                admin_provider_ops_network_error_message(&err),
                None,
            );
        }
    };
    let response_time_ms = Some(start.elapsed().as_millis() as u64);

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

fn admin_provider_ops_network_error_message(error: &str) -> String {
    let normalized = error.trim();
    let lower = normalized.to_ascii_lowercase();
    if lower.contains("timeout") || normalized.contains("超时") {
        return "请求超时".to_string();
    }
    format!("网络错误: {normalized}")
}
