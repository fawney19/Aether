mod checkin;
mod query_balance;
mod responses;
mod support;

use super::config::{
    admin_provider_ops_config_object, admin_provider_ops_connector_object,
    admin_provider_ops_decrypted_credentials, resolve_admin_provider_ops_base_url,
};
use super::support::ADMIN_PROVIDER_OPS_ACTION_RUST_ONLY_MESSAGE;
use super::verify::admin_provider_ops_verify_headers;
use crate::handlers::admin::request::AdminAppState;
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
    _provider_id: &str,
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
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("generic_api");
    let connector_config = admin_provider_ops_connector_object(provider_ops_config)
        .and_then(|connector| connector.get("config"))
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    if support::admin_provider_ops_should_use_rust_only_action_stub(
        architecture_id,
        &connector_config,
    ) {
        return responses::admin_provider_ops_action_not_supported(
            action_type,
            ADMIN_PROVIDER_OPS_ACTION_RUST_ONLY_MESSAGE,
        );
    }

    let Some(base_url) =
        resolve_admin_provider_ops_base_url(provider, endpoints, Some(provider_ops_config))
    else {
        return responses::admin_provider_ops_action_not_configured(
            action_type,
            "Provider 未配置 base_url",
        );
    };
    let credentials = admin_provider_ops_decrypted_credentials(
        state,
        admin_provider_ops_config_object(provider)
            .and_then(admin_provider_ops_connector_object)
            .and_then(|connector| connector.get("credentials")),
    );
    let headers =
        match admin_provider_ops_verify_headers(architecture_id, &connector_config, &credentials) {
            Ok(headers) => headers,
            Err(message) => {
                return responses::admin_provider_ops_action_not_configured(action_type, message);
            }
        };
    let Some(action_config) = support::admin_provider_ops_resolved_action_config(
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
            query_balance::admin_provider_ops_run_query_balance_action(
                state,
                &base_url,
                architecture_id,
                &action_config,
                &headers,
                &credentials,
            )
            .await
        }
        "checkin" => {
            let has_cookie = credentials
                .get("cookie")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|value| !value.trim().is_empty());
            checkin::admin_provider_ops_run_checkin_action(
                state,
                &base_url,
                architecture_id,
                &action_config,
                &headers,
                has_cookie,
            )
            .await
        }
        _ => responses::admin_provider_ops_action_not_supported(
            action_type,
            ADMIN_PROVIDER_OPS_ACTION_RUST_ONLY_MESSAGE,
        ),
    }
}
