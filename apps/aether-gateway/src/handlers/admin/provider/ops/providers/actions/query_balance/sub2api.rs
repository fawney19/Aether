use super::super::super::config::persist_admin_provider_ops_runtime_credentials;
use super::super::super::verify::{
    admin_provider_ops_execute_json_request, admin_provider_ops_sub2api_exchange_token,
    admin_provider_ops_sub2api_request_url, AdminProviderOpsExecuteJsonError,
};
use super::super::responses::{
    admin_provider_ops_action_error, admin_provider_ops_action_response,
};
use super::super::support::admin_provider_ops_json_object_map;
use crate::handlers::admin::request::AdminAppState;
use aether_admin::provider::ops::parse_sub2api_balance_payload;
use aether_contracts::ProxySnapshot;
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider;
use serde_json::{json, Value};
use tracing::warn;

pub(super) async fn admin_provider_ops_sub2api_balance_payload(
    state: &AdminAppState<'_>,
    provider_id: &str,
    provider: &StoredProviderCatalogProvider,
    base_url: &str,
    action_config: &serde_json::Map<String, serde_json::Value>,
    credentials: &serde_json::Map<String, serde_json::Value>,
    proxy_snapshot: Option<&ProxySnapshot>,
) -> serde_json::Value {
    let start = std::time::Instant::now();
    let (access_token, updated_credentials, _frontend_updated_credentials) =
        match admin_provider_ops_sub2api_exchange_token(
            state,
            base_url,
            credentials,
            proxy_snapshot,
        )
        .await
        {
            Ok(value) => value,
            Err(message) => {
                return admin_provider_ops_action_error(
                    "auth_failed",
                    "query_balance",
                    message,
                    None,
                )
            }
        };

    if !updated_credentials.is_empty() {
        if let Err(err) =
            persist_admin_provider_ops_runtime_credentials(state, provider, &updated_credentials)
                .await
        {
            warn!(
                provider_id = %provider_id,
                error = ?err,
                "failed to persist sub2api runtime credentials"
            );
        }
    }

    let me_endpoint = action_config
        .get("endpoint")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("/api/v1/auth/me?timezone=Asia/Shanghai");
    let me_url = admin_provider_ops_sub2api_request_url(base_url, me_endpoint);
    let subscription_endpoint = admin_provider_ops_json_object_map(json!({
        "endpoint": action_config
            .get("subscription_endpoint")
            .cloned()
            .unwrap_or_else(|| json!("/api/v1/subscriptions/summary")),
    }))
    .get("endpoint")
    .and_then(serde_json::Value::as_str)
    .map(str::trim)
    .filter(|value| !value.is_empty())
    .unwrap_or("/api/v1/subscriptions/summary")
    .to_string();
    let subscription_url =
        admin_provider_ops_sub2api_request_url(base_url, subscription_endpoint.as_str());

    let auth_value = match reqwest::header::HeaderValue::from_str(&format!("Bearer {access_token}"))
    {
        Ok(value) => value,
        Err(_) => {
            return admin_provider_ops_action_error(
                "parse_error",
                "query_balance",
                "访问令牌格式无效",
                None,
            );
        }
    };
    let auth_headers =
        reqwest::header::HeaderMap::from_iter([(reqwest::header::AUTHORIZATION, auth_value)]);
    let me_request_id = format!("provider-ops-action:sub2api:me:{provider_id}");
    let subscription_request_id =
        format!("provider-ops-action:sub2api:subscriptions:{provider_id}");
    let (me_result, subscription_result) = tokio::join!(
        admin_provider_ops_execute_json_request(
            state,
            &me_request_id,
            reqwest::Method::GET,
            &me_url,
            &auth_headers,
            None,
            proxy_snapshot,
        ),
        admin_provider_ops_execute_json_request(
            state,
            &subscription_request_id,
            reqwest::Method::GET,
            &subscription_url,
            &auth_headers,
            None,
            proxy_snapshot,
        )
    );
    let me_result = me_result.map_err(|err| match err {
        AdminProviderOpsExecuteJsonError::InvalidJson(message)
        | AdminProviderOpsExecuteJsonError::Transport(message) => message,
    });
    let subscription_result = subscription_result.map_err(|err| match err {
        AdminProviderOpsExecuteJsonError::InvalidJson(message)
        | AdminProviderOpsExecuteJsonError::Transport(message) => message,
    });
    let response_time_ms = Some(start.elapsed().as_millis() as u64);

    let (me_status, me_json) = match me_result {
        Ok(result) => result,
        Err(err) => {
            return admin_provider_ops_action_error(
                "network_error",
                "query_balance",
                network_error_message(&err),
                response_time_ms,
            );
        }
    };
    if matches!(
        me_status,
        http::StatusCode::UNAUTHORIZED | http::StatusCode::FORBIDDEN
    ) {
        return admin_provider_ops_action_error(
            "auth_failed",
            "query_balance",
            "认证失败，请检查凭据配置",
            response_time_ms,
        );
    }
    if me_status != http::StatusCode::OK {
        return admin_provider_ops_action_error(
            "unknown_error",
            "query_balance",
            format!(
                "HTTP {}: {}",
                me_status.as_u16(),
                me_status.canonical_reason().unwrap_or("Unknown")
            ),
            response_time_ms,
        );
    }

    let subscription_json = subscription_result
        .ok()
        .and_then(|(status, payload)| (status == http::StatusCode::OK).then_some(payload));
    let data =
        match parse_sub2api_balance_payload(action_config, &me_json, subscription_json.as_ref()) {
            Ok(data) => data,
            Err(message) => {
                return admin_provider_ops_action_error(
                    if message == "响应格式无效" {
                        "parse_error"
                    } else {
                        "unknown_error"
                    },
                    "query_balance",
                    message,
                    response_time_ms,
                );
            }
        };

    admin_provider_ops_action_response(
        "success",
        "query_balance",
        data,
        None,
        response_time_ms,
        86400,
    )
}

fn network_error_message(error: &str) -> String {
    let normalized = error.trim();
    let lower = normalized.to_ascii_lowercase();
    if lower.contains("timeout") || normalized.contains("超时") {
        return "请求超时".to_string();
    }
    if normalized.starts_with("网络错误:") {
        return normalized.to_string();
    }
    format!("网络错误: {normalized}")
}
