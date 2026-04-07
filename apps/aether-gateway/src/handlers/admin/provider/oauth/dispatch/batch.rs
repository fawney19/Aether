use super::super::refresh::{
    build_internal_control_error_response, build_provider_oauth_auth_config_from_token_payload,
    create_provider_oauth_catalog_key, find_duplicate_provider_oauth_key,
    provider_oauth_active_api_formats, provider_oauth_key_proxy_value,
    refresh_provider_oauth_account_state_after_update, update_existing_provider_oauth_catalog_key,
};
use super::super::state::{
    admin_provider_oauth_template, build_admin_provider_oauth_backend_unavailable_response,
    current_unix_secs, decode_jwt_claims, exchange_admin_provider_oauth_refresh_token,
    is_fixed_provider_type_for_provider_oauth, save_provider_oauth_batch_task_payload,
};
use crate::control::GatewayPublicRequestContext;
use crate::handlers::admin::provider::shared::paths::{
    admin_provider_oauth_batch_import_provider_id,
    admin_provider_oauth_batch_import_task_provider_id,
};
use crate::handlers::admin::provider::shared::support::ADMIN_PROVIDER_OAUTH_DATA_UNAVAILABLE_DETAIL;
use crate::{AppState, GatewayError};
use axum::{
    body::{to_bytes, Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::warn;
use uuid::Uuid;

const PROVIDER_OAUTH_BATCH_TASK_MAX_ERROR_SAMPLES: usize = 20;

#[derive(Debug, Clone, Deserialize)]
struct AdminProviderOAuthBatchImportRequest {
    credentials: String,
    proxy_node_id: Option<String>,
}

#[derive(Debug, Clone)]
struct AdminProviderOAuthBatchImportEntry {
    refresh_token: String,
    account_id: Option<String>,
    account_user_id: Option<String>,
    plan_type: Option<String>,
    user_id: Option<String>,
    email: Option<String>,
}

#[derive(Debug, Clone)]
struct AdminProviderOAuthBatchImportOutcome {
    total: usize,
    success: usize,
    failed: usize,
    results: Vec<serde_json::Value>,
}

fn build_kiro_batch_import_key_name(
    email: Option<&str>,
    auth_method: Option<&str>,
    refresh_token: Option<&str>,
) -> String {
    let method = auth_method
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("social");
    let base = email
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| {
            let mut hasher = Sha256::new();
            hasher.update(refresh_token.unwrap_or_default().as_bytes());
            let digest = hasher.finalize();
            let hex = digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            format!("kiro_{}", &hex[..6])
        });
    format!("{base} ({method})")
}

fn parse_admin_provider_oauth_batch_import_request(
    request_body: Option<&Bytes>,
) -> Result<AdminProviderOAuthBatchImportRequest, Response<Body>> {
    let Some(request_body) = request_body else {
        return Err(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "请求体必须是合法的 JSON 对象",
        ));
    };
    match serde_json::from_slice::<AdminProviderOAuthBatchImportRequest>(request_body) {
        Ok(payload) if !payload.credentials.trim().is_empty() => Ok(payload),
        _ => Err(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "请求体必须是合法的 JSON 对象",
        )),
    }
}

fn coerce_admin_provider_oauth_import_str(value: Option<&serde_json::Value>) -> Option<String> {
    value
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn extract_admin_provider_oauth_batch_import_entry(
    item: &serde_json::Value,
) -> Option<AdminProviderOAuthBatchImportEntry> {
    match item {
        serde_json::Value::String(value) => {
            let refresh_token = value.trim();
            if refresh_token.is_empty() {
                None
            } else {
                Some(AdminProviderOAuthBatchImportEntry {
                    refresh_token: refresh_token.to_string(),
                    account_id: None,
                    account_user_id: None,
                    plan_type: None,
                    user_id: None,
                    email: None,
                })
            }
        }
        serde_json::Value::Object(object) => {
            let refresh_token = coerce_admin_provider_oauth_import_str(
                object
                    .get("refresh_token")
                    .or_else(|| object.get("refreshToken")),
            )?;
            let account_id = coerce_admin_provider_oauth_import_str(
                object
                    .get("account_id")
                    .or_else(|| object.get("accountId"))
                    .or_else(|| object.get("chatgpt_account_id"))
                    .or_else(|| object.get("chatgptAccountId")),
            );
            let account_user_id = coerce_admin_provider_oauth_import_str(
                object
                    .get("account_user_id")
                    .or_else(|| object.get("accountUserId"))
                    .or_else(|| object.get("chatgpt_account_user_id"))
                    .or_else(|| object.get("chatgptAccountUserId")),
            );
            let plan_type = coerce_admin_provider_oauth_import_str(
                object
                    .get("plan_type")
                    .or_else(|| object.get("planType"))
                    .or_else(|| object.get("chatgpt_plan_type"))
                    .or_else(|| object.get("chatgptPlanType")),
            )
            .map(|value| value.to_ascii_lowercase());
            let user_id = coerce_admin_provider_oauth_import_str(
                object
                    .get("user_id")
                    .or_else(|| object.get("userId"))
                    .or_else(|| object.get("chatgpt_user_id"))
                    .or_else(|| object.get("chatgptUserId")),
            );
            let email = coerce_admin_provider_oauth_import_str(object.get("email"));
            Some(AdminProviderOAuthBatchImportEntry {
                refresh_token,
                account_id,
                account_user_id,
                plan_type,
                user_id,
                email,
            })
        }
        _ => None,
    }
}

fn parse_admin_provider_oauth_batch_import_entries(
    raw_credentials: &str,
) -> Vec<AdminProviderOAuthBatchImportEntry> {
    let raw = raw_credentials.trim();
    if raw.is_empty() {
        return Vec::new();
    }

    if raw.starts_with('[') {
        if let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(raw)
        {
            return items
                .iter()
                .filter_map(extract_admin_provider_oauth_batch_import_entry)
                .collect();
        }
    }

    if raw.starts_with('{') {
        if let Ok(value @ serde_json::Value::Object(_)) =
            serde_json::from_str::<serde_json::Value>(raw)
        {
            return extract_admin_provider_oauth_batch_import_entry(&value)
                .into_iter()
                .collect();
        }
    }

    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|refresh_token| AdminProviderOAuthBatchImportEntry {
            refresh_token: refresh_token.to_string(),
            account_id: None,
            account_user_id: None,
            plan_type: None,
            user_id: None,
            email: None,
        })
        .collect()
}

fn apply_admin_provider_oauth_batch_import_hints(
    provider_type: &str,
    entry: &AdminProviderOAuthBatchImportEntry,
    auth_config: &mut serde_json::Map<String, serde_json::Value>,
) {
    if !provider_type.eq_ignore_ascii_case("codex") {
        return;
    }
    if let Some(account_id) = entry.account_id.as_ref() {
        auth_config
            .entry("account_id".to_string())
            .or_insert_with(|| json!(account_id));
    }
    if let Some(account_user_id) = entry.account_user_id.as_ref() {
        auth_config
            .entry("account_user_id".to_string())
            .or_insert_with(|| json!(account_user_id));
    }
    if let Some(plan_type) = entry.plan_type.as_ref() {
        auth_config
            .entry("plan_type".to_string())
            .or_insert_with(|| json!(plan_type));
    }
    if let Some(user_id) = entry.user_id.as_ref() {
        auth_config
            .entry("user_id".to_string())
            .or_insert_with(|| json!(user_id));
    }
    if let Some(email) = entry.email.as_ref() {
        auth_config
            .entry("email".to_string())
            .or_insert_with(|| json!(email));
    }
}

async fn extract_admin_provider_oauth_batch_error_detail(response: Response<Body>) -> String {
    let status = response.status();
    let raw_body = to_bytes(response.into_body(), usize::MAX).await.ok();
    if let Some(raw_body) = raw_body {
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&raw_body) {
            if let Some(detail) = value.get("detail").and_then(serde_json::Value::as_str) {
                let normalized = detail.trim();
                if !normalized.is_empty() {
                    return normalized.to_string();
                }
            }
        }
        let normalized = String::from_utf8_lossy(&raw_body).trim().to_string();
        if !normalized.is_empty() {
            return normalized;
        }
    }
    format!("HTTP {}", status.as_u16())
}

fn normalize_admin_provider_oauth_kiro_import_item(
    item: &serde_json::Value,
) -> Option<serde_json::Value> {
    match item {
        serde_json::Value::String(value) => {
            let refresh_token = value.trim();
            if refresh_token.is_empty() {
                None
            } else {
                Some(json!({ "refresh_token": refresh_token }))
            }
        }
        serde_json::Value::Object(object) => {
            if let Some(nested) = object
                .get("auth_config")
                .or_else(|| object.get("authConfig"))
                .and_then(serde_json::Value::as_object)
            {
                let mut merged = nested.clone();
                for key in [
                    "provider_type",
                    "providerType",
                    "auth_method",
                    "authMethod",
                    "auth_type",
                    "authType",
                    "refresh_token",
                    "refreshToken",
                    "expires_at",
                    "expiresAt",
                    "profile_arn",
                    "profileArn",
                    "region",
                    "auth_region",
                    "authRegion",
                    "api_region",
                    "apiRegion",
                    "client_id",
                    "clientId",
                    "client_secret",
                    "clientSecret",
                    "machine_id",
                    "machineId",
                    "kiro_version",
                    "kiroVersion",
                    "system_version",
                    "systemVersion",
                    "node_version",
                    "nodeVersion",
                    "email",
                    "access_token",
                    "accessToken",
                ] {
                    if let Some(value) = object.get(key) {
                        if !value.is_null()
                            && !(value.is_string()
                                && value.as_str().is_some_and(|inner| inner.trim().is_empty()))
                        {
                            merged.insert(key.to_string(), value.clone());
                        }
                    }
                }
                return Some(serde_json::Value::Object(merged));
            }
            Some(serde_json::Value::Object(object.clone()))
        }
        _ => None,
    }
}

fn parse_admin_provider_oauth_kiro_batch_import_entries(
    raw_credentials: &str,
) -> Vec<serde_json::Value> {
    let raw = raw_credentials.trim();
    if raw.is_empty() {
        return Vec::new();
    }

    if raw.starts_with('[') {
        if let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(raw)
        {
            return items
                .iter()
                .filter_map(normalize_admin_provider_oauth_kiro_import_item)
                .collect();
        }
    }

    if raw.starts_with('{') {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) {
            return normalize_admin_provider_oauth_kiro_import_item(&value)
                .into_iter()
                .collect();
        }
    }

    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|refresh_token| json!({ "refreshToken": refresh_token }))
        .collect()
}

fn admin_provider_oauth_kiro_refresh_base_url_override(
    state: &AppState,
    override_key: &str,
) -> Option<String> {
    let override_url = state.provider_oauth_token_url(override_key, "");
    let normalized = override_url.trim();
    (!normalized.is_empty()).then(|| normalized.to_string())
}

async fn execute_admin_provider_oauth_kiro_batch_import(
    state: &AppState,
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
    let key_proxy = provider_oauth_key_proxy_value(proxy_node_id);
    let adapter = crate::provider_transport::kiro::KiroOAuthRefreshAdapter::default()
        .with_refresh_base_urls(
            admin_provider_oauth_kiro_refresh_base_url_override(state, "kiro_social_refresh"),
            admin_provider_oauth_kiro_refresh_base_url_override(state, "kiro_idc_refresh"),
        );
    let mut results = Vec::with_capacity(entries.len());
    let mut success = 0usize;
    let mut failed = 0usize;

    for (index, entry) in entries.iter().enumerate() {
        let Some(mut refreshed_auth_config) =
            crate::provider_transport::kiro::KiroAuthConfig::from_json_value(entry)
        else {
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

        refreshed_auth_config = match adapter
            .refresh_auth_config(&state.client, &refreshed_auth_config)
            .await
        {
            Ok(config) => config,
            Err(err) => {
                failed += 1;
                results.push(json!({
                    "index": index,
                    "status": "error",
                    "error": format!("Token 验证失败: {err:?}"),
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
        .and_then(|claims| claims.get("email").cloned())
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
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
            .map(ToOwned::to_owned);
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
                &access_token,
                &auth_config,
                key_proxy.clone(),
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
                email.as_deref(),
                refreshed_auth_config.auth_method.as_deref(),
                refreshed_auth_config.refresh_token.as_deref(),
            );
            match create_provider_oauth_catalog_key(
                state,
                provider_id,
                key_name.as_str(),
                &access_token,
                &auth_config,
                &api_formats,
                key_proxy.clone(),
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

        let _ =
            refresh_provider_oauth_account_state_after_update(state, &provider, &persisted_key.id)
                .await;

        success += 1;
        results.push(json!({
            "index": index,
            "status": "success",
            "key_id": persisted_key.id,
            "key_name": key_name,
            "auth_method": refreshed_auth_config
                .auth_method
                .clone()
                .unwrap_or_else(|| "social".to_string()),
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

fn estimate_admin_provider_oauth_batch_import_total(
    provider_type: &str,
    raw_credentials: &str,
) -> usize {
    if provider_type.eq_ignore_ascii_case("kiro") {
        parse_admin_provider_oauth_kiro_batch_import_entries(raw_credentials).len()
    } else {
        parse_admin_provider_oauth_batch_import_entries(raw_credentials).len()
    }
}

async fn execute_admin_provider_oauth_batch_import_for_provider_type(
    state: &AppState,
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

async fn execute_admin_provider_oauth_batch_import(
    state: &AppState,
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

fn build_admin_provider_oauth_batch_import_response(
    outcome: AdminProviderOAuthBatchImportOutcome,
) -> Response<Body> {
    Json(json!({
        "total": outcome.total,
        "success": outcome.success,
        "failed": outcome.failed,
        "results": outcome.results,
    }))
    .into_response()
}

fn build_admin_provider_oauth_batch_task_state(
    task_id: &str,
    provider_id: &str,
    provider_type: &str,
    status: &str,
    total: usize,
    processed: usize,
    success: usize,
    failed: usize,
    message: Option<&str>,
    error: Option<&str>,
    error_samples: Vec<serde_json::Value>,
    created_at: u64,
    started_at: Option<u64>,
    finished_at: Option<u64>,
) -> serde_json::Value {
    let updated_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(created_at);
    let progress_percent = if total == 0 {
        0
    } else {
        ((processed * 100) / total).min(100) as u64
    };
    json!({
        "task_id": task_id,
        "provider_id": provider_id,
        "provider_type": provider_type,
        "status": status,
        "total": total,
        "processed": processed,
        "success": success,
        "failed": failed,
        "progress_percent": progress_percent,
        "message": message,
        "error": error,
        "error_samples": error_samples,
        "created_at": created_at,
        "started_at": started_at,
        "finished_at": finished_at,
        "updated_at": updated_at,
    })
}

pub(super) async fn handle_admin_provider_oauth_batch_import(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_provider_catalog_data_reader() {
        return Ok(build_admin_provider_oauth_backend_unavailable_response());
    }
    let Some(provider_id) =
        admin_provider_oauth_batch_import_provider_id(&request_context.request_path)
    else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::NOT_FOUND,
            "Provider 不存在",
        ));
    };
    let payload = match parse_admin_provider_oauth_batch_import_request(request_body) {
        Ok(payload) => payload,
        Err(response) => return Ok(response),
    };

    let Some(provider) = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::NOT_FOUND,
            "Provider 不存在",
        ));
    };
    let provider_type = provider.provider_type.trim().to_ascii_lowercase();
    if !is_fixed_provider_type_for_provider_oauth(&provider_type) {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "该 Provider 不是固定类型，无法使用 provider-oauth",
        ));
    }
    if provider_type != "kiro" && admin_provider_oauth_template(&provider_type).is_none() {
        return Ok(build_admin_provider_oauth_backend_unavailable_response());
    }

    let total = estimate_admin_provider_oauth_batch_import_total(
        &provider_type,
        payload.credentials.as_str(),
    );
    if total == 0 {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "未找到有效的 Token 数据",
        ));
    }

    let outcome = execute_admin_provider_oauth_batch_import_for_provider_type(
        state,
        &provider_id,
        &provider_type,
        payload.credentials.as_str(),
        payload.proxy_node_id.as_deref(),
    )
    .await?;
    Ok(build_admin_provider_oauth_batch_import_response(outcome))
}

pub(super) async fn handle_admin_provider_oauth_start_batch_import_task(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&axum::body::Bytes>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_provider_catalog_data_reader() {
        return Ok(build_admin_provider_oauth_backend_unavailable_response());
    }
    let Some(provider_id) =
        admin_provider_oauth_batch_import_task_provider_id(&request_context.request_path)
    else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::NOT_FOUND,
            "Provider 不存在",
        ));
    };
    let payload = match parse_admin_provider_oauth_batch_import_request(request_body) {
        Ok(payload) => payload,
        Err(response) => return Ok(response),
    };

    let Some(provider) = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::NOT_FOUND,
            "Provider 不存在",
        ));
    };
    let provider_type = provider.provider_type.trim().to_ascii_lowercase();
    if !is_fixed_provider_type_for_provider_oauth(&provider_type) {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "该 Provider 不是固定类型，无法使用 provider-oauth",
        ));
    }
    if provider_type != "kiro" && admin_provider_oauth_template(&provider_type).is_none() {
        return Ok(build_admin_provider_oauth_backend_unavailable_response());
    }

    let total = estimate_admin_provider_oauth_batch_import_total(
        &provider_type,
        payload.credentials.as_str(),
    );
    if total == 0 {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "未找到有效的 Token 数据",
        ));
    }

    let task_id = Uuid::new_v4().to_string();
    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let submitted_state = build_admin_provider_oauth_batch_task_state(
        &task_id,
        &provider_id,
        &provider_type,
        "submitted",
        total,
        0,
        0,
        0,
        Some("任务已提交，等待执行"),
        None,
        Vec::new(),
        created_at,
        None,
        None,
    );
    if save_provider_oauth_batch_task_payload(state, &task_id, &submitted_state)
        .await
        .is_err()
    {
        return Ok(build_internal_control_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            "provider oauth batch task redis unavailable",
        ));
    }

    let task_state = state.clone();
    let task_id_for_worker = task_id.clone();
    let provider_id_for_worker = provider_id.clone();
    let provider_type_for_worker = provider_type.clone();
    let proxy_node_id = payload.proxy_node_id.clone();
    let raw_credentials = payload.credentials.clone();
    tokio::spawn(async move {
        let started_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs())
            .unwrap_or(created_at);
        let processing_state = build_admin_provider_oauth_batch_task_state(
            &task_id_for_worker,
            &provider_id_for_worker,
            &provider_type_for_worker,
            "processing",
            total,
            0,
            0,
            0,
            Some("任务开始执行"),
            None,
            Vec::new(),
            created_at,
            Some(started_at),
            None,
        );
        let _ = save_provider_oauth_batch_task_payload(
            &task_state,
            &task_id_for_worker,
            &processing_state,
        )
        .await;

        match execute_admin_provider_oauth_batch_import_for_provider_type(
            &task_state,
            &provider_id_for_worker,
            &provider_type_for_worker,
            raw_credentials.as_str(),
            proxy_node_id.as_deref(),
        )
        .await
        {
            Ok(outcome) => {
                let finished_at = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .ok()
                    .map(|duration| duration.as_secs())
                    .unwrap_or(started_at);
                let error_samples = outcome
                    .results
                    .iter()
                    .filter(|item| {
                        item.get("status").and_then(serde_json::Value::as_str) == Some("error")
                    })
                    .take(PROVIDER_OAUTH_BATCH_TASK_MAX_ERROR_SAMPLES)
                    .cloned()
                    .collect::<Vec<_>>();
                let message = format!(
                    "导入完成：成功 {}，失败 {}",
                    outcome.success, outcome.failed
                );
                let completed_state = build_admin_provider_oauth_batch_task_state(
                    &task_id_for_worker,
                    &provider_id_for_worker,
                    &provider_type_for_worker,
                    "completed",
                    outcome.total,
                    outcome.total,
                    outcome.success,
                    outcome.failed,
                    Some(message.as_str()),
                    None,
                    error_samples,
                    created_at,
                    Some(started_at),
                    Some(finished_at),
                );
                let _ = save_provider_oauth_batch_task_payload(
                    &task_state,
                    &task_id_for_worker,
                    &completed_state,
                )
                .await;
            }
            Err(err) => {
                let finished_at = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .ok()
                    .map(|duration| duration.as_secs())
                    .unwrap_or(started_at);
                let error_message = format!("{err:?}");
                let failed_state = build_admin_provider_oauth_batch_task_state(
                    &task_id_for_worker,
                    &provider_id_for_worker,
                    &provider_type_for_worker,
                    "failed",
                    total,
                    0,
                    0,
                    0,
                    Some("导入任务执行失败"),
                    Some(error_message.as_str()),
                    Vec::new(),
                    created_at,
                    Some(started_at),
                    Some(finished_at),
                );
                let _ = save_provider_oauth_batch_task_payload(
                    &task_state,
                    &task_id_for_worker,
                    &failed_state,
                )
                .await;
                warn!(
                    task_id = %task_id_for_worker,
                    provider_id = %provider_id_for_worker,
                    error = %error_message,
                    "provider oauth batch import task failed"
                );
            }
        }
    });

    Ok(Json(json!({
        "task_id": task_id,
        "status": "submitted",
        "total": total,
        "processed": 0,
        "success": 0,
        "failed": 0,
        "progress_percent": 0,
        "message": "任务已提交，正在后台导入",
    }))
    .into_response())
}
