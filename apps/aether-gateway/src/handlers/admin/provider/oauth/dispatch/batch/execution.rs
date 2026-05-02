use super::super::token_import::build_codex_access_token_import_auth_config;
use super::kiro_import::execute_admin_provider_oauth_kiro_batch_import;
use super::parse::{
    apply_admin_provider_oauth_batch_import_hints, extract_admin_provider_oauth_batch_error_detail,
    parse_admin_provider_oauth_batch_import_entries, AdminProviderOAuthBatchImportEntry,
    AdminProviderOAuthBatchImportOutcome,
};
use super::progress::{
    maybe_report_admin_provider_oauth_batch_import_progress,
    AdminProviderOAuthBatchProgressReporter,
};
use crate::handlers::admin::provider::oauth::duplicates::find_duplicate_provider_oauth_key;
use crate::handlers::admin::provider::oauth::provisioning::build_provider_oauth_auth_config_from_token_payload;
use crate::handlers::admin::provider::oauth::provisioning::{
    create_provider_oauth_catalog_key, provider_oauth_active_api_formats,
    provider_oauth_key_proxy_value, update_existing_provider_oauth_catalog_key,
};
use crate::handlers::admin::provider::oauth::runtime::{
    provider_oauth_runtime_endpoint_for_provider,
    spawn_provider_oauth_account_state_refresh_after_update,
};
use crate::handlers::admin::provider::oauth::state::{
    admin_provider_oauth_template, exchange_admin_provider_oauth_refresh_token,
};
use crate::handlers::admin::provider::shared::support::ADMIN_PROVIDER_OAUTH_DATA_UNAVAILABLE_DETAIL;
use crate::handlers::admin::request::{AdminAppState, AdminProviderOAuthTemplate};
use crate::GatewayError;
use aether_admin::provider::oauth::parse_admin_provider_oauth_kiro_batch_import_entries;
use aether_contracts::ProxySnapshot;
use serde_json::{json, Map, Value};
use std::time::{SystemTime, UNIX_EPOCH};

struct AdminProviderOAuthResolvedBatchImport {
    access_token: String,
    auth_config: Map<String, Value>,
    expires_at: Option<u64>,
}

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
    progress: Option<&mut dyn AdminProviderOAuthBatchProgressReporter>,
) -> Result<AdminProviderOAuthBatchImportOutcome, GatewayError> {
    if provider_type.eq_ignore_ascii_case("kiro") {
        execute_admin_provider_oauth_kiro_batch_import(
            state,
            provider_id,
            raw_credentials,
            proxy_node_id,
            progress,
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
            progress,
        )
        .await
    }
}

async fn resolve_admin_provider_oauth_batch_import_tokens(
    state: &AdminAppState<'_>,
    template: AdminProviderOAuthTemplate,
    provider_type: &str,
    entry: &AdminProviderOAuthBatchImportEntry,
    request_proxy: Option<ProxySnapshot>,
) -> Result<AdminProviderOAuthResolvedBatchImport, String> {
    let refresh_token = entry
        .refresh_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let access_token = entry
        .access_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    if let Some(refresh_token) = refresh_token {
        let token_payload = match exchange_admin_provider_oauth_refresh_token(
            state,
            template,
            refresh_token,
            request_proxy.clone(),
        )
        .await
        {
            Ok(payload) => payload,
            Err(response) => {
                let detail = extract_admin_provider_oauth_batch_error_detail(response).await;
                if provider_type.eq_ignore_ascii_case("codex") {
                    if let Some(access_token) = access_token {
                        let (auth_config, expires_at) = build_codex_access_token_import_auth_config(
                            access_token,
                            Some(refresh_token),
                            entry.expires_at,
                            Some(detail.as_str()),
                        );
                        return Ok(AdminProviderOAuthResolvedBatchImport {
                            access_token: access_token.to_string(),
                            auth_config,
                            expires_at,
                        });
                    }
                }
                return Err(format!("Token 验证失败: {detail}"));
            }
        };

        let (mut auth_config, access_token, returned_refresh_token, expires_at) =
            build_provider_oauth_auth_config_from_token_payload(provider_type, &token_payload);
        let Some(access_token) = access_token else {
            return Err("Token 刷新返回缺少 access_token".to_string());
        };

        let refresh_token = returned_refresh_token
            .or_else(|| Some(refresh_token.to_string()))
            .filter(|value| !value.trim().is_empty());
        if let Some(refresh_token) = refresh_token.as_ref() {
            auth_config.insert("refresh_token".to_string(), json!(refresh_token));
        }
        return Ok(AdminProviderOAuthResolvedBatchImport {
            access_token,
            auth_config,
            expires_at,
        });
    }

    if let Some(access_token) = access_token {
        if !provider_type.eq_ignore_ascii_case("codex") {
            return Err("Access Token 导入仅支持 Codex Provider".to_string());
        }
        let (auth_config, expires_at) =
            build_codex_access_token_import_auth_config(access_token, None, entry.expires_at, None);
        return Ok(AdminProviderOAuthResolvedBatchImport {
            access_token: access_token.to_string(),
            auth_config,
            expires_at,
        });
    }

    Err("Refresh Token 或 Access Token 不能为空".to_string())
}

pub(super) async fn execute_admin_provider_oauth_batch_import(
    state: &AdminAppState<'_>,
    provider_id: &str,
    provider_type: &str,
    entries: &[AdminProviderOAuthBatchImportEntry],
    proxy_node_id: Option<&str>,
    mut progress: Option<&mut dyn AdminProviderOAuthBatchProgressReporter>,
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
    let runtime_endpoint = provider_oauth_runtime_endpoint_for_provider(provider_type, &endpoints);
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
    let key_proxy = provider_oauth_key_proxy_value(proxy_node_id);
    let mut results = Vec::with_capacity(entries.len());
    let mut success = 0usize;
    let mut failed = 0usize;

    for (index, entry) in entries.iter().enumerate() {
        let resolved_import = match resolve_admin_provider_oauth_batch_import_tokens(
            state,
            template,
            provider_type,
            entry,
            request_proxy.clone(),
        )
        .await
        {
            Ok(value) => value,
            Err(error) => {
                failed += 1;
                results.push(json!({
                    "index": index,
                    "status": "error",
                    "error": error,
                    "replaced": false,
                }));
                maybe_report_admin_provider_oauth_batch_import_progress(
                    &mut progress,
                    entries.len(),
                    success,
                    failed,
                    &results,
                )
                .await;
                continue;
            }
        };
        let AdminProviderOAuthResolvedBatchImport {
            access_token,
            mut auth_config,
            expires_at,
        } = resolved_import;
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
                    maybe_report_admin_provider_oauth_batch_import_progress(
                        &mut progress,
                        entries.len(),
                        success,
                        failed,
                        &results,
                    )
                    .await;
                    continue;
                }
            };

        let replaced = duplicate.is_some();
        let (persisted_key, key_name) = if let Some(existing_key) = duplicate {
            match update_existing_provider_oauth_catalog_key(
                state,
                &existing_key,
                provider_type,
                &access_token,
                &auth_config,
                &api_formats,
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
                    maybe_report_admin_provider_oauth_batch_import_progress(
                        &mut progress,
                        entries.len(),
                        success,
                        failed,
                        &results,
                    )
                    .await;
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
                provider_type,
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
                    maybe_report_admin_provider_oauth_batch_import_progress(
                        &mut progress,
                        entries.len(),
                        success,
                        failed,
                        &results,
                    )
                    .await;
                    continue;
                }
            }
        };

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
            "error": serde_json::Value::Null,
            "replaced": replaced,
        }));
        maybe_report_admin_provider_oauth_batch_import_progress(
            &mut progress,
            entries.len(),
            success,
            failed,
            &results,
        )
        .await;
    }

    Ok(AdminProviderOAuthBatchImportOutcome {
        total: entries.len(),
        success,
        failed,
        results,
    })
}
