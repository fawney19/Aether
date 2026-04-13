mod sub2api;
mod yescode;

use super::super::support::AdminProviderOpsCheckinOutcome;
use super::super::verify::{
    admin_provider_ops_execute_json_request, AdminProviderOpsExecuteJsonError,
};
use super::checkin::admin_provider_ops_probe_new_api_checkin;
use super::responses::{admin_provider_ops_action_error, admin_provider_ops_action_response};
use super::support::{admin_provider_ops_request_method, admin_provider_ops_request_url};
use crate::handlers::admin::request::AdminAppState;
use aether_admin::provider::ops::{
    attach_balance_checkin_outcome, parse_query_balance_payload, ProviderOpsArchitectureSpec,
    ProviderOpsBalanceMode, ProviderOpsCheckinMode,
};
use aether_contracts::ProxySnapshot;
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider;

pub(super) async fn admin_provider_ops_run_query_balance_action(
    state: &AdminAppState<'_>,
    provider_id: &str,
    provider: &StoredProviderCatalogProvider,
    architecture: &ProviderOpsArchitectureSpec,
    base_url: &str,
    action_config: &serde_json::Map<String, serde_json::Value>,
    headers: &reqwest::header::HeaderMap,
    credentials: &serde_json::Map<String, serde_json::Value>,
    proxy_snapshot: Option<&ProxySnapshot>,
) -> serde_json::Value {
    match architecture.balance_mode {
        ProviderOpsBalanceMode::YescodeCombined => {
            return yescode::admin_provider_ops_yescode_balance_payload(
                state,
                base_url,
                headers,
                action_config,
                proxy_snapshot,
            )
            .await;
        }
        ProviderOpsBalanceMode::Sub2ApiDualRequest => {
            return sub2api::admin_provider_ops_sub2api_balance_payload(
                state,
                provider_id,
                provider,
                base_url,
                action_config,
                credentials,
                proxy_snapshot,
            )
            .await;
        }
        ProviderOpsBalanceMode::SingleRequest => {}
    }

    let mut balance_checkin = None::<AdminProviderOpsCheckinOutcome>;
    if architecture.checkin_mode == ProviderOpsCheckinMode::NewApiCompatible {
        let has_cookie = ["session_cookie", "cookie"].into_iter().any(|key| {
            credentials
                .get(key)
                .and_then(serde_json::Value::as_str)
                .is_some_and(|value| !value.trim().is_empty())
        });
        balance_checkin = admin_provider_ops_probe_new_api_checkin(
            state,
            base_url,
            action_config,
            headers,
            has_cookie,
            proxy_snapshot,
        )
        .await;
    }

    let start = std::time::Instant::now();
    let url = admin_provider_ops_request_url(base_url, action_config, "/api/user/balance");
    let method = admin_provider_ops_request_method(action_config, "GET");
    let (status, response_json) = match admin_provider_ops_execute_json_request(
        state,
        &format!(
            "provider-ops-action:{}:query_balance:{provider_id}",
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
                "query_balance",
                "响应不是有效的 JSON",
                Some(start.elapsed().as_millis() as u64),
            );
        }
        Err(AdminProviderOpsExecuteJsonError::Transport(err)) => {
            return admin_provider_ops_action_error(
                "network_error",
                "query_balance",
                admin_provider_ops_network_error_message(&err),
                None,
            );
        }
    };
    let response_time_ms = Some(start.elapsed().as_millis() as u64);

    if status != http::StatusCode::OK {
        let cookie_auth = architecture.query_balance_cookie_auth_errors;
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

    let data = match parse_query_balance_payload(
        architecture.architecture_id,
        action_config,
        &response_json,
    ) {
        Ok(data) => data,
        Err(message) => {
            return admin_provider_ops_action_error(
                if architecture.architecture_id == "generic_api"
                    || architecture.architecture_id == "new_api"
                    || architecture.architecture_id == "anyrouter"
                {
                    "unknown_error"
                } else {
                    "parse_error"
                },
                "query_balance",
                message,
                response_time_ms,
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
        attach_balance_checkin_outcome(&mut payload, outcome);
    }
    payload
}

fn admin_provider_ops_network_error_message(error: &str) -> String {
    let normalized = error.trim();
    let lower = normalized.to_ascii_lowercase();
    if lower.contains("timeout") || normalized.contains("超时") {
        return "请求超时".to_string();
    }
    format!("网络错误: {normalized}")
}
