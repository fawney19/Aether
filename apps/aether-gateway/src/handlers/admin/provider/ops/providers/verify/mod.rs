mod proxy;
mod request;
mod sub2api;

use crate::handlers::admin::request::AdminAppState;
use aether_admin::provider::ops::{
    admin_provider_ops_verify_failure, build_headers, get_architecture, normalize_architecture_id,
    parse_verify_payload, ProviderOpsVerifyMode,
};
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider;

pub(super) use proxy::{
    admin_provider_ops_anyrouter_acw_cookie, admin_provider_ops_resolve_proxy_snapshot,
};
pub(super) use request::{
    admin_provider_ops_execute_json_request, admin_provider_ops_execute_proxy_json_request,
    AdminProviderOpsExecuteJsonError,
};
pub(super) use sub2api::{
    admin_provider_ops_sub2api_exchange_token, admin_provider_ops_sub2api_request_url,
};

pub(super) async fn admin_provider_ops_local_verify_response(
    state: &AdminAppState<'_>,
    provider: Option<&StoredProviderCatalogProvider>,
    base_url: &str,
    architecture_id: &str,
    config: &serde_json::Map<String, serde_json::Value>,
    credentials: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Value {
    let architecture_id = normalize_architecture_id(architecture_id);
    let Some(architecture) = get_architecture(architecture_id) else {
        return admin_provider_ops_verify_failure("认证验证仅支持 Rust execution runtime");
    };

    let base_url = base_url.trim().trim_end_matches('/');
    if base_url.is_empty() {
        return admin_provider_ops_verify_failure("请提供 API 地址");
    }

    let proxy_snapshot =
        proxy::admin_provider_ops_resolve_proxy_snapshot(state, Some(config)).await;
    if architecture.verify_mode == ProviderOpsVerifyMode::Sub2ApiExchange {
        return sub2api::admin_provider_ops_local_sub2api_verify_response(
            state,
            provider,
            base_url,
            architecture.verify_endpoint,
            credentials,
            proxy_snapshot.as_ref(),
        )
        .await;
    }

    let mut resolved_config = config.clone();
    if architecture.architecture_id == "anyrouter" {
        if let Some(challenge) =
            proxy::admin_provider_ops_anyrouter_acw_cookie(state, base_url, Some(config)).await
        {
            resolved_config.insert(
                "acw_cookie".to_string(),
                serde_json::Value::String(challenge.acw_cookie),
            );
        }
    }

    let headers = match build_headers(architecture.architecture_id, &resolved_config, credentials) {
        Ok(headers) => headers,
        Err(message) => return admin_provider_ops_verify_failure(message),
    };
    let verify_url = format!("{base_url}{}", architecture.verify_endpoint);
    let (status, response_json) = match request::admin_provider_ops_execute_get_json(
        state,
        &format!("provider-ops-verify:{}", architecture.architecture_id),
        &verify_url,
        &headers,
        proxy_snapshot.as_ref(),
    )
    .await
    {
        Ok(result) => result,
        Err(error) => {
            return admin_provider_ops_verify_failure(
                request::admin_provider_ops_verify_execution_error_message(&error),
            );
        }
    };

    parse_verify_payload(architecture.architecture_id, status, &response_json, None)
}
