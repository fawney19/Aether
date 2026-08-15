mod checkin;
mod query_balance;
mod responses;
mod support;

use super::config::{
    admin_provider_ops_config_object, admin_provider_ops_connector_object,
    admin_provider_ops_decrypted_credentials, resolve_admin_provider_ops_base_url,
};
use super::support::ADMIN_PROVIDER_OPS_ACTION_RUST_ONLY_MESSAGE;
use super::verify::{
    admin_provider_ops_anyrouter_acw_cookie, admin_provider_ops_resolve_proxy_snapshot,
};
use crate::handlers::admin::request::AdminAppState;
use aether_admin::provider::ops::{
    build_headers, get_architecture, normalize_architecture_id, resolve_action_config,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogProvider,
};

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

pub(crate) async fn admin_provider_ops_local_action_response(
    state: &AdminAppState<'_>,
    provider_id: &str,
    provider: Option<&StoredProviderCatalogProvider>,
    endpoints: &[StoredProviderCatalogEndpoint],
    action_type: &str,
    request_config: Option<&serde_json::Map<String, serde_json::Value>>,
) -> serde_json::Value {
    let Some(provider) = provider else {
        return responses::admin_provider_ops_action_not_configured(action_type, "未配置操作设置");
    };
    let Some(provider_ops_config) = admin_provider_ops_config_object(provider) else {
        return responses::admin_provider_ops_action_not_configured(action_type, "未配置操作设置");
    };
    let architecture_id = provider_ops_config
        .get("architecture_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("generic_api");
    let architecture_id = normalize_architecture_id(architecture_id);
    let Some(architecture) = get_architecture(architecture_id) else {
        return responses::admin_provider_ops_action_not_supported(
            action_type,
            ADMIN_PROVIDER_OPS_ACTION_RUST_ONLY_MESSAGE,
        );
    };
    let Some(base_url) =
        resolve_admin_provider_ops_base_url(provider, endpoints, Some(provider_ops_config))
    else {
        return responses::admin_provider_ops_action_not_configured(
            action_type,
            "Provider 未配置 base_url",
        );
    };

    let mut connector_config = admin_provider_ops_connector_object(provider_ops_config)
        .and_then(|connector| connector.get("config"))
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    if architecture_id == "anyrouter" {
        if let Some(challenge) =
            admin_provider_ops_anyrouter_acw_cookie(state, &base_url, Some(&connector_config)).await
        {
            connector_config.insert(
                "acw_cookie".to_string(),
                serde_json::Value::String(challenge.acw_cookie),
            );
        }
    }
    let proxy_snapshot =
        admin_provider_ops_resolve_proxy_snapshot(state, Some(&connector_config)).await;

    let credentials = admin_provider_ops_decrypted_credentials(
        state,
        admin_provider_ops_config_object(provider)
            .and_then(admin_provider_ops_connector_object)
            .and_then(|connector| connector.get("credentials")),
    );
    let headers = match build_headers(
        architecture.architecture_id,
        &connector_config,
        &credentials,
    ) {
        Ok(headers) => headers,
        Err(message) => {
            return responses::admin_provider_ops_action_not_configured(action_type, message);
        }
    };
    let Some(action_config) = resolve_action_config(
        architecture_id,
        provider_ops_config,
        action_type,
        request_config,
    ) else {
        return responses::admin_provider_ops_action_not_supported(
            action_type,
            ADMIN_PROVIDER_OPS_ACTION_RUST_ONLY_MESSAGE,
        );
    };

    match action_type {
        "query_balance" => {
            let remote_quota_config = if architecture_id == "sub2api" {
                match aether_admin::provider::ops::parse_sub2api_remote_quota_config(
                    provider_ops_config,
                ) {
                    Ok(config) => config,
                    Err(message) => {
                        return responses::admin_provider_ops_action_not_configured(
                            action_type,
                            message,
                        );
                    }
                }
            } else {
                None
            };
            query_balance::admin_provider_ops_run_query_balance_action(
                state,
                provider_id,
                provider,
                &architecture,
                &base_url,
                &action_config,
                &headers,
                &credentials,
                proxy_snapshot.as_ref(),
                remote_quota_config.as_ref(),
            )
            .await
        }
        "checkin" => {
            let has_cookie = ["cookie", "session_cookie"].into_iter().any(|key| {
                credentials
                    .get(key)
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|value| !value.trim().is_empty())
            });
            checkin::admin_provider_ops_run_checkin_action(
                state,
                &base_url,
                &architecture,
                &action_config,
                &headers,
                has_cookie,
                proxy_snapshot.as_ref(),
            )
            .await
        }
        _ => responses::admin_provider_ops_action_not_supported(
            action_type,
            ADMIN_PROVIDER_OPS_ACTION_RUST_ONLY_MESSAGE,
        ),
    }
}
