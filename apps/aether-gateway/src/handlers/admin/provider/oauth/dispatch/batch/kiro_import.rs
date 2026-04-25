use super::super::kiro::{
    admin_provider_oauth_kiro_refresh_base_url_override,
    refresh_admin_provider_oauth_kiro_auth_config,
};
use super::parse::AdminProviderOAuthBatchImportOutcome;
use crate::handlers::admin::provider::oauth::duplicates::find_duplicate_provider_oauth_key;
use crate::handlers::admin::provider::oauth::provisioning::{
    create_provider_oauth_catalog_key, provider_oauth_active_api_formats,
    update_existing_provider_oauth_catalog_key,
};
use crate::handlers::admin::provider::oauth::runtime::{
    provider_oauth_runtime_endpoint_for_provider,
    spawn_provider_oauth_account_state_refresh_after_update,
};
use crate::handlers::admin::provider::oauth::state::decode_jwt_claims;
use crate::handlers::admin::provider::shared::support::ADMIN_PROVIDER_OAUTH_DATA_UNAVAILABLE_DETAIL;
use crate::handlers::admin::request::{AdminAppState, AdminKiroAuthConfig};
use crate::GatewayError;
use aether_admin::provider::oauth::{
    build_kiro_batch_import_key_name, coerce_admin_provider_oauth_import_str,
    parse_admin_provider_oauth_kiro_batch_import_entries,
};
use serde_json::{json, Map, Value};

pub(super) async fn execute_admin_provider_oauth_kiro_batch_import(
    state: &AdminAppState<'_>,
    provider_id: &str,
    raw_credentials: &str,
    proxy_node_id: Option<&str>,
) -> Result<AdminProviderOAuthBatchImportOutcome, GatewayError> {
    let entries = parse_admin_provider_oauth_kiro_batch_import_entries(raw_credentials);
    let Some(provider) = state
        .read_provider_catalog_providers_by_ids(&[provider_id.to_string()])
        .await?
        .into_iter()
        .next()
    else {
        return Ok(AdminProviderOAuthBatchImportOutcome {
            total: entries.len(),
            success: 0,
            failed: entries.len(),
            results: entries
                .iter()
                .enumerate()
                .map(|(index, _)| {
                    json!({
                        "index": index,
                        "status": "error",
                        "error": "Provider 不存在",
                        "replaced": false,
                    })
                })
                .collect(),
        });
    };

    let endpoints = state
        .list_provider_catalog_endpoints_by_provider_ids(&[provider_id.to_string()])
        .await?;
    let api_formats = provider_oauth_active_api_formats(&endpoints);
    let runtime_endpoint = provider_oauth_runtime_endpoint_for_provider("kiro", &endpoints);
    let request_proxy = state
        .resolve_admin_provider_oauth_operation_proxy_snapshot(
            proxy_node_id,
            &[
                runtime_endpoint
                    .as_ref()
                    .and_then(|endpoint| endpoint.proxy.as_ref()),
                provider.proxy.as_ref(),
            ],
        )
        .await;
    let social_refresh_base_url =
        admin_provider_oauth_kiro_refresh_base_url_override(state, "kiro_social_refresh");
    let idc_refresh_base_url =
        admin_provider_oauth_kiro_refresh_base_url_override(state, "kiro_idc_refresh");
    let mut results = Vec::with_capacity(entries.len());
    let mut success = 0usize;
    let mut failed = 0usize;

    for (index, entry) in entries.iter().enumerate() {
        let Some(mut refreshed_auth_config) = AdminKiroAuthConfig::from_json_value(entry) else {
            failed += 1;
            results.push(json!({
                "index": index,
                "status": "error",
                "error": "未找到有效的凭据数据",
                "replaced": false,
            }));
            continue;
        };

        let has_refresh_token = refreshed_auth_config
            .refresh_token
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty());
        if !has_refresh_token {
            failed += 1;
            results.push(json!({
                "index": index,
                "status": "error",
                "error": "缺少可用的 Kiro refresh 凭据",
                "replaced": false,
            }));
            continue;
        }

        refreshed_auth_config = match refresh_admin_provider_oauth_kiro_auth_config(
            state,
            &refreshed_auth_config,
            request_proxy.clone(),
            social_refresh_base_url.as_deref(),
            idc_refresh_base_url.as_deref(),
        )
        .await
        {
            Ok(config) => config,
            Err(err) => {
                failed += 1;
                results.push(json!({
                    "index": index,
                    "status": "error",
                    "error": format!("Token 验证失败: {err}"),
                    "replaced": false,
                }));
                continue;
            }
        };

        if refreshed_auth_config.auth_method.is_none() {
            refreshed_auth_config.auth_method = Some(if refreshed_auth_config.is_idc_auth() {
                "idc".to_string()
            } else {
                "social".to_string()
            });
        }

        let mut auth_config = refreshed_auth_config
            .to_json_value()
            .as_object()
            .cloned()
            .unwrap_or_default();
        auth_config.insert("provider_type".to_string(), json!("kiro"));
        let email = decode_jwt_claims(
            refreshed_auth_config
                .access_token
                .as_deref()
                .unwrap_or_default(),
        )
        .and_then(|claims: Map<String, Value>| claims.get("email").cloned())
        .and_then(|value: Value| value.as_str().map(ToOwned::to_owned))
        .or_else(|| coerce_admin_provider_oauth_import_str(entry.get("email")));
        if let Some(email) = email.as_ref() {
            auth_config.insert("email".to_string(), json!(email));
        }

        let duplicate =
            match find_duplicate_provider_oauth_key(state, provider_id, &auth_config, None).await {
                Ok(value) => value,
                Err(detail) => {
                    failed += 1;
                    results.push(json!({
                        "index": index,
                        "status": "error",
                        "error": detail,
                        "replaced": false,
                    }));
                    continue;
                }
            };

        let access_token = refreshed_auth_config
            .access_token
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_string());
        let Some(access_token) = access_token else {
            failed += 1;
            results.push(json!({
                "index": index,
                "status": "error",
                "error": "Token 验证失败: accessToken 为空",
                "replaced": false,
            }));
            continue;
        };

        let replaced = duplicate.is_some();
        let (persisted_key, key_name) = if let Some(existing_key) = duplicate {
            match update_existing_provider_oauth_catalog_key(
                state,
                &existing_key,
                provider.provider_type.as_str(),
                &access_token,
                &auth_config,
                &api_formats,
                None,
                refreshed_auth_config.expires_at,
            )
            .await?
            {
                Some(key) => (key, existing_key.name.clone()),
                None => {
                    failed += 1;
                    results.push(json!({
                        "index": index,
                        "status": "error",
                        "error": "provider oauth write unavailable",
                        "replaced": true,
                    }));
                    continue;
                }
            }
        } else {
            let key_name = build_kiro_batch_import_key_name(
                auth_config.get("email").and_then(serde_json::Value::as_str),
                auth_config
                    .get("auth_method")
                    .and_then(serde_json::Value::as_str),
                auth_config
                    .get("refresh_token")
                    .and_then(serde_json::Value::as_str),
            );
            match create_provider_oauth_catalog_key(
                state,
                provider_id,
                provider.provider_type.as_str(),
                &key_name,
                &access_token,
                &auth_config,
                &api_formats,
                None,
                refreshed_auth_config.expires_at,
            )
            .await?
            {
                Some(key) => (key, key_name),
                None => {
                    failed += 1;
                    results.push(json!({
                        "index": index,
                        "status": "error",
                        "error": "provider oauth write unavailable",
                        "replaced": false,
                    }));
                    continue;
                }
            }
        };

        let auth_method = auth_config
            .get("auth_method")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        spawn_provider_oauth_account_state_refresh_after_update(
            state.cloned_app(),
            provider.clone(),
            persisted_key.id.clone(),
            request_proxy.clone(),
        );

        success += 1;
        results.push(json!({
            "index": index,
            "status": "success",
            "key_id": persisted_key.id,
            "key_name": key_name,
            "auth_method": auth_method,
            "error": serde_json::Value::Null,
            "replaced": replaced,
        }));
    }

    Ok(AdminProviderOAuthBatchImportOutcome {
        total: entries.len(),
        success,
        failed,
        results,
    })
}
