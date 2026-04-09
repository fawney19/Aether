mod parsers;
mod yescode;

use super::super::support::{
    AdminProviderOpsCheckinOutcome, ADMIN_PROVIDER_OPS_ACTION_RUST_ONLY_MESSAGE,
};
use super::checkin::admin_provider_ops_probe_new_api_checkin;
use super::responses::{
    admin_provider_ops_action_error, admin_provider_ops_action_not_supported,
    admin_provider_ops_action_response,
};
use super::support::{
    admin_provider_ops_is_cookie_auth_architecture, admin_provider_ops_request_method,
    admin_provider_ops_request_url,
};
use crate::handlers::admin::request::AdminAppState;

pub(super) async fn admin_provider_ops_run_query_balance_action(
    state: &AdminAppState<'_>,
    base_url: &str,
    architecture_id: &str,
    action_config: &serde_json::Map<String, serde_json::Value>,
    headers: &reqwest::header::HeaderMap,
    credentials: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Value {
    if architecture_id == "yescode" {
        return yescode::admin_provider_ops_yescode_balance_payload(
            state,
            base_url,
            headers,
            action_config,
        )
        .await;
    }

    let mut balance_checkin = None::<AdminProviderOpsCheckinOutcome>;
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
        .http_client()
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
        return match status {
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
    }

    let data = match architecture_id {
        "generic_api" | "new_api" => {
            match parsers::admin_provider_ops_new_api_balance_payload(action_config, &response_json)
            {
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
            match parsers::admin_provider_ops_cubence_balance_payload(action_config, &response_json)
            {
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
        "nekocode" => match parsers::admin_provider_ops_nekocode_balance_payload(&response_json) {
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
        parsers::admin_provider_ops_attach_balance_checkin_outcome(&mut payload, outcome);
    }
    payload
}
