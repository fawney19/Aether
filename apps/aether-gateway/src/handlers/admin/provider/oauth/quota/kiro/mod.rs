mod parse;
mod plan;

use self::parse::parse_kiro_usage_response;
use self::plan::execute_kiro_quota_plan;
use super::shared::{
    build_quota_snapshot_payload, extract_execution_error_message,
    persist_provider_quota_refresh_state, quota_refresh_success_invalid_state,
    ProviderQuotaExecutionOutcome,
};
use crate::handlers::admin::request::{AdminAppState, AdminLocalOAuthRefreshError};
use crate::GatewayError;
use aether_contracts::ProxySnapshot;
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_provider_transport::kiro::build_kiro_request_auth_from_config;
use aether_provider_transport::{CachedOAuthEntry, LocalResolvedOAuthRequestAuth};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

fn kiro_quota_error_is_token_invalid(detail: Option<&str>) -> bool {
    let Some(detail) = detail else {
        return false;
    };
    let normalized = detail.to_ascii_lowercase();
    normalized.contains("bearer token invalid")
        || normalized.contains("bearer token invild")
        || normalized.contains("bearer token is invalid")
        || normalized.contains("invalid bearer token")
        || normalized.contains("token expired")
        || normalized.contains("token has expired")
        || normalized.contains("expired token")
}

fn kiro_quota_error_is_account_banned(detail: Option<&str>) -> bool {
    let Some(detail) = detail else {
        return false;
    };
    let normalized = detail.to_ascii_lowercase();
    [
        "account suspended",
        "account is suspended",
        "account banned",
        "account is banned",
        "terms of service",
        "封禁",
        "封号",
        "被封",
        "账户已封禁",
    ]
    .iter()
    .any(|keyword| normalized.contains(keyword))
}

fn kiro_auth_from_refreshed_entry(
    entry: &CachedOAuthEntry,
) -> Option<LocalResolvedOAuthRequestAuth> {
    if !entry.provider_type.trim().eq_ignore_ascii_case("kiro") {
        return None;
    }
    let auth_config = entry
        .metadata
        .as_ref()
        .and_then(aether_provider_transport::kiro::KiroAuthConfig::from_json_value)?;
    let auth = build_kiro_request_auth_from_config(auth_config, None)?;
    Some(LocalResolvedOAuthRequestAuth::Kiro(auth))
}

fn kiro_quota_refresh_failure_status(err: &AdminLocalOAuthRefreshError) -> Option<u16> {
    match err {
        AdminLocalOAuthRefreshError::HttpStatus { status_code, .. } => Some(*status_code),
        _ => None,
    }
}

fn kiro_quota_refresh_failure_message(err: &AdminLocalOAuthRefreshError) -> String {
    match err {
        AdminLocalOAuthRefreshError::HttpStatus {
            status_code,
            body_excerpt,
            ..
        } => format!("Kiro Token 刷新失败 ({status_code}): {body_excerpt}"),
        _ => format!("Kiro Token 刷新失败: {err}"),
    }
}

pub(crate) async fn refresh_kiro_provider_quota_locally(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    endpoint: &StoredProviderCatalogEndpoint,
    keys: Vec<StoredProviderCatalogKey>,
    proxy_override: Option<ProxySnapshot>,
) -> Result<Option<serde_json::Value>, GatewayError> {
    let mut results = Vec::new();
    let mut success_count = 0usize;
    let mut failed_count = 0usize;

    for key in keys {
        let transport = match state
            .read_provider_transport_snapshot(&provider.id, &endpoint.id, &key.id)
            .await?
        {
            Some(transport) => transport,
            None => {
                failed_count += 1;
                results.push(json!({
                    "key_id": key.id,
                    "key_name": key.name,
                    "status": "error",
                    "message": "Provider transport snapshot unavailable",
                }));
                continue;
            }
        };

        let auth = match state.force_local_oauth_refresh_entry(&transport).await {
            Ok(Some(entry)) => match kiro_auth_from_refreshed_entry(&entry) {
                Some(LocalResolvedOAuthRequestAuth::Kiro(auth)) => auth,
                _ => {
                    failed_count += 1;
                    results.push(json!({
                        "key_id": key.id,
                        "key_name": key.name,
                        "status": "error",
                        "message": "Kiro Token 刷新成功但认证信息解析失败",
                    }));
                    continue;
                }
            },
            Ok(None) => match state
                .resolve_local_oauth_kiro_request_auth(&transport)
                .await?
            {
                Some(auth) => auth,
                None => {
                    failed_count += 1;
                    results.push(json!({
                        "key_id": key.id,
                        "key_name": key.name,
                        "status": "error",
                        "message": "缺少 Kiro 认证配置 (auth_config)",
                    }));
                    continue;
                }
            },
            Err(err) => {
                failed_count += 1;
                let mut payload = serde_json::Map::new();
                payload.insert("key_id".to_string(), json!(key.id));
                payload.insert("key_name".to_string(), json!(key.name));
                payload.insert("status".to_string(), json!("error"));
                payload.insert(
                    "message".to_string(),
                    json!(kiro_quota_refresh_failure_message(&err)),
                );
                if let Some(status_code) = kiro_quota_refresh_failure_status(&err) {
                    payload.insert("status_code".to_string(), json!(status_code));
                }
                results.push(serde_json::Value::Object(payload));
                continue;
            }
        };

        let result =
            match execute_kiro_quota_plan(state, &transport, &auth, proxy_override.as_ref()).await?
            {
                ProviderQuotaExecutionOutcome::Response(result) => result,
                ProviderQuotaExecutionOutcome::Failure(detail) => {
                    failed_count += 1;
                    results.push(json!({
                        "key_id": key.id,
                        "key_name": key.name,
                        "status": "error",
                        "message": format!("getUsageLimits 请求执行失败: {detail}"),
                        "status_code": 502,
                    }));
                    continue;
                }
            };

        let now_unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let mut metadata_update = None::<serde_json::Value>;
        let mut encrypted_auth_config = None::<String>;
        let (mut oauth_invalid_at_unix_secs, mut oauth_invalid_reason) =
            quota_refresh_success_invalid_state(&key);
        let mut status = "error".to_string();
        let mut message = None::<String>;

        if result.status_code == 200 {
            if let Some(body_json) = result
                .body
                .as_ref()
                .and_then(|body| body.json_body.as_ref())
            {
                metadata_update = parse_kiro_usage_response(body_json, now_unix_secs)
                    .map(|metadata| json!({ "kiro": metadata }));
                if metadata_update.is_some() {
                    let mut auth_config_object = transport
                        .key
                        .decrypted_auth_config
                        .as_deref()
                        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
                        .and_then(|value| value.as_object().cloned())
                        .unwrap_or_default();
                    if let Some(refreshed_auth_config) =
                        auth.auth_config.to_json_value().as_object()
                    {
                        for (key, value) in refreshed_auth_config {
                            auth_config_object.insert(key.clone(), value.clone());
                        }
                    }
                    auth_config_object
                        .entry("provider_type".to_string())
                        .or_insert_with(|| json!("kiro"));
                    let auth_config_json =
                        serde_json::Value::Object(auth_config_object).to_string();
                    if let Some(auth_config_json) =
                        state.encrypt_catalog_secret_with_fallbacks(auth_config_json.as_str())
                    {
                        encrypted_auth_config = Some(auth_config_json);
                    }
                    status = "success".to_string();
                } else {
                    status = "no_metadata".to_string();
                    message = Some("响应中未包含限额信息".to_string());
                }
            } else {
                status = "no_metadata".to_string();
                message = Some("响应中未包含限额信息".to_string());
            }
        } else {
            let err_msg = extract_execution_error_message(&result);
            message = Some(match err_msg.as_deref() {
                Some(detail) if !detail.is_empty() => {
                    format!(
                        "getUsageLimits 返回状态码 {}: {}",
                        result.status_code, detail
                    )
                }
                _ => format!("getUsageLimits 返回状态码 {}", result.status_code),
            });
            match result.status_code {
                401 => {
                    oauth_invalid_at_unix_secs = Some(now_unix_secs);
                    oauth_invalid_reason = Some("Kiro Token 无效或已过期".to_string());
                }
                403 | 423 => {
                    let reason = err_msg
                        .clone()
                        .filter(|value| !value.trim().is_empty())
                        .unwrap_or_else(|| format!("HTTP {}", result.status_code));
                    if kiro_quota_error_is_token_invalid(err_msg.as_deref()) {
                        oauth_invalid_at_unix_secs = Some(now_unix_secs);
                        oauth_invalid_reason = Some("Kiro Token 无效或已过期".to_string());
                    } else if kiro_quota_error_is_account_banned(err_msg.as_deref()) {
                        oauth_invalid_at_unix_secs = Some(now_unix_secs);
                        oauth_invalid_reason = Some(format!("账户已封禁: {reason}"));
                        metadata_update = Some(json!({
                            "kiro": {
                                "is_banned": true,
                                "ban_reason": reason,
                                "banned_at": now_unix_secs,
                                "updated_at": now_unix_secs,
                            }
                        }));
                        status = "banned".to_string();
                    }
                }
                _ => {}
            }
        }

        if !persist_provider_quota_refresh_state(
            state,
            &key.id,
            metadata_update.as_ref(),
            oauth_invalid_at_unix_secs,
            oauth_invalid_reason,
            encrypted_auth_config,
        )
        .await?
        {
            failed_count += 1;
            results.push(json!({
                "key_id": key.id,
                "key_name": key.name,
                "status": "error",
                "message": "Key 状态写入失败",
            }));
            continue;
        }

        if status == "success" {
            success_count += 1;
        } else {
            failed_count += 1;
        }

        let mut payload = serde_json::Map::new();
        payload.insert("key_id".to_string(), json!(key.id));
        payload.insert("key_name".to_string(), json!(key.name));
        payload.insert("status".to_string(), json!(status));
        if let Some(message) = message {
            payload.insert("message".to_string(), json!(message));
        }
        if let Some(metadata) = metadata_update
            .as_ref()
            .and_then(|value| value.get("kiro"))
            .cloned()
        {
            payload.insert("metadata".to_string(), metadata);
        }
        if let Some(quota_snapshot) = build_quota_snapshot_payload(
            "kiro",
            key.status_snapshot.as_ref(),
            metadata_update.as_ref(),
        ) {
            payload.insert("quota_snapshot".to_string(), quota_snapshot);
        }
        results.push(serde_json::Value::Object(payload));
    }

    Ok(Some(json!({
        "success": success_count,
        "failed": failed_count,
        "total": success_count + failed_count,
        "results": results,
        "message": format!("已处理 {} 个 Key", success_count + failed_count),
        "auto_removed": 0,
    })))
}

#[cfg(test)]
mod tests {
    use super::{kiro_quota_error_is_account_banned, kiro_quota_error_is_token_invalid};

    #[test]
    fn bearer_token_invalid_is_not_classified_as_banned() {
        let detail = Some("Bearer token invalid");

        assert!(kiro_quota_error_is_token_invalid(detail));
        assert!(!kiro_quota_error_is_account_banned(detail));
    }

    #[test]
    fn bearer_token_invild_typo_is_not_classified_as_banned() {
        let detail = Some("bearer token invild");

        assert!(kiro_quota_error_is_token_invalid(detail));
        assert!(!kiro_quota_error_is_account_banned(detail));
    }

    #[test]
    fn explicit_kiro_account_suspension_is_classified_as_banned() {
        let detail = Some("account suspended due to Terms of Service violation");

        assert!(!kiro_quota_error_is_token_invalid(detail));
        assert!(kiro_quota_error_is_account_banned(detail));
    }
}
