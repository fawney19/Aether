use super::kiro_import::execute_admin_provider_oauth_kiro_batch_import;
use super::parse::{
    apply_admin_provider_oauth_batch_import_hints, extract_admin_provider_oauth_batch_error_detail,
    parse_admin_provider_oauth_batch_import_entries, AdminProviderOAuthBatchImportEntry,
    AdminProviderOAuthBatchImportOutcome,
};
use crate::handlers::admin::provider::oauth::duplicates::find_duplicate_provider_oauth_key;
use crate::handlers::admin::provider::oauth::provisioning::build_provider_oauth_auth_config_from_token_payload;
use crate::handlers::admin::provider::oauth::provisioning::{
    create_provider_oauth_catalog_key, provider_oauth_active_api_formats,
    provider_oauth_key_proxy_value, update_existing_provider_oauth_catalog_key,
};
use crate::handlers::admin::provider::oauth::runtime::refresh_provider_oauth_account_state_after_update;
use crate::handlers::admin::provider::oauth::state::{
    admin_provider_oauth_template, exchange_admin_provider_oauth_refresh_token,
};
use crate::handlers::admin::provider::shared::support::ADMIN_PROVIDER_OAUTH_DATA_UNAVAILABLE_DETAIL;
use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use aether_admin::provider::oauth::parse_admin_provider_oauth_kiro_batch_import_entries;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn estimate_admin_provider_oauth_batch_import_total(
    provider_type: &str,
    raw_credentials: &str,
) -> usize {
    if provider_type.eq_ignore_ascii_case("kiro") {
        parse_admin_provider_oauth_kiro_batch_import_entries(raw_credentials).len()
    } else {
        parse_admin_provider_oauth_batch_import_entries(raw_credentials).len()
    }
}

pub(super) async fn execute_admin_provider_oauth_batch_import_for_provider_type(
    state: &AdminAppState<'_>,
    provider_id: &str,
    provider_type: &str,
    raw_credentials: &str,
    proxy_node_id: Option<&str>,
) -> Result<AdminProviderOAuthBatchImportOutcome, GatewayError> {
    if provider_type.eq_ignore_ascii_case("kiro") {
        execute_admin_provider_oauth_kiro_batch_import(
            state,
            provider_id,
            raw_credentials,
            proxy_node_id,
        )
        .await
    } else {
        let entries = parse_admin_provider_oauth_batch_import_entries(raw_credentials);
        execute_admin_provider_oauth_batch_import(
            state,
            provider_id,
            provider_type,
            &entries,
            proxy_node_id,
        )
        .await
    }
}

pub(super) async fn execute_admin_provider_oauth_batch_import(
    state: &AdminAppState<'_>,
    provider_id: &str,
    provider_type: &str,
    entries: &[AdminProviderOAuthBatchImportEntry],
    proxy_node_id: Option<&str>,
) -> Result<AdminProviderOAuthBatchImportOutcome, GatewayError> {
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

    let Some(template) = admin_provider_oauth_template(provider_type) else {
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
                        "error": ADMIN_PROVIDER_OAUTH_DATA_UNAVAILABLE_DETAIL,
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
    let key_proxy = provider_oauth_key_proxy_value(proxy_node_id);
    let mut results = Vec::with_capacity(entries.len());
    let mut success = 0usize;
    let mut failed = 0usize;

    for (index, entry) in entries.iter().enumerate() {
        let token_payload = match exchange_admin_provider_oauth_refresh_token(
            state,
            template,
            entry.refresh_token.as_str(),
            proxy_node_id,
        )
        .await
        {
            Ok(payload) => payload,
            Err(response) => {
                failed += 1;
                results.push(json!({
                    "index": index,
                    "status": "error",
                    "error": format!(
                        "Token 验证失败: {}",
                        extract_admin_provider_oauth_batch_error_detail(response).await
                    ),
                    "replaced": false,
                }));
                continue;
            }
        };

        let (mut auth_config, access_token, returned_refresh_token, expires_at) =
            build_provider_oauth_auth_config_from_token_payload(provider_type, &token_payload);
        let Some(access_token) = access_token else {
            failed += 1;
            results.push(json!({
                "index": index,
                "status": "error",
                "error": "Token 刷新返回缺少 access_token",
                "replaced": false,
            }));
            continue;
        };

        let refresh_token = returned_refresh_token
            .or_else(|| Some(entry.refresh_token.clone()))
            .filter(|value| !value.trim().is_empty());
        if let Some(refresh_token) = refresh_token.as_ref() {
            auth_config.insert("refresh_token".to_string(), json!(refresh_token));
        }
        apply_admin_provider_oauth_batch_import_hints(provider_type, entry, &mut auth_config);

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

        let replaced = duplicate.is_some();
        let (persisted_key, key_name) = if let Some(existing_key) = duplicate {
            match update_existing_provider_oauth_catalog_key(
                state,
                &existing_key,
                &access_token,
                &auth_config,
                key_proxy.clone(),
                expires_at,
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
            let key_name = auth_config
                .get("email")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|email| format!("{provider_type}_{email}"))
                .unwrap_or_else(|| {
                    format!(
                        "{}_{}_{}",
                        provider_type,
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .ok()
                            .map(|duration| duration.as_secs())
                            .unwrap_or(0),
                        index
                    )
                });
            match create_provider_oauth_catalog_key(
                state,
                provider_id,
                key_name.as_str(),
                &access_token,
                &auth_config,
                &api_formats,
                key_proxy.clone(),
                expires_at,
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

        let _ =
            refresh_provider_oauth_account_state_after_update(state, &provider, &persisted_key.id)
                .await;

        success += 1;
        results.push(json!({
            "index": index,
            "status": "success",
            "key_id": persisted_key.id,
            "key_name": key_name,
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
