use super::shared::{
    build_quota_snapshot_payload, default_provider_quota_execution_timeouts,
    execute_provider_quota_plan, extract_execution_error_message,
    oauth_refresh_auto_removed_result, persist_provider_quota_refresh_state,
    quota_key_auto_removed, quota_refresh_success_invalid_state, ProviderQuotaExecutionOutcome,
};
use crate::handlers::admin::provider::shared::payloads::{
    OAUTH_ACCOUNT_BLOCK_PREFIX, OAUTH_EXPIRED_PREFIX, OAUTH_REFRESH_FAILED_PREFIX,
};
use crate::handlers::admin::request::{AdminAppState, AdminGatewayProviderTransportSnapshot};
use crate::GatewayError;
use aether_admin::provider::quota::parse_chatgpt_web_conversation_init_response;
use aether_contracts::{
    ExecutionResult, ProxySnapshot, ResolvedTransportProfile, TRANSPORT_BACKEND_BROWSER_WREQ,
    TRANSPORT_HTTP_MODE_AUTO, TRANSPORT_POOL_SCOPE_KEY,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_provider_pool::{
    build_chatgpt_web_pool_quota_request, enrich_chatgpt_web_quota_metadata,
    normalize_chatgpt_web_image_quota_limit,
};
use base64::Engine as _;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

const PLACEHOLDER_API_KEY: &str = "__placeholder__";
const CHATGPT_WEB_BROWSER_PROFILE: &str = "chrome143";

fn chatgpt_web_auth_config(
    transport: &AdminGatewayProviderTransportSnapshot,
) -> Option<serde_json::Value> {
    transport
        .key
        .decrypted_auth_config
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok())
}

async fn resolve_chatgpt_web_quota_auth(
    state: &AdminAppState<'_>,
    transport: &AdminGatewayProviderTransportSnapshot,
) -> Result<Option<(String, String)>, GatewayError> {
    if let Some(auth) = state.resolve_local_oauth_header_auth(transport).await? {
        return Ok(Some(auth));
    }
    let decrypted_key = transport.key.decrypted_api_key.trim();
    if decrypted_key.is_empty() || decrypted_key == PLACEHOLDER_API_KEY {
        return Ok(None);
    }
    Ok(Some((
        "authorization".to_string(),
        format!("Bearer {decrypted_key}"),
    )))
}

async fn execute_chatgpt_web_quota_plan(
    state: &AdminAppState<'_>,
    transport: &AdminGatewayProviderTransportSnapshot,
    endpoint: &StoredProviderCatalogEndpoint,
    authorization: (String, String),
    proxy_override: Option<&ProxySnapshot>,
) -> Result<ProviderQuotaExecutionOutcome, GatewayError> {
    let proxy = match proxy_override {
        Some(proxy) => Some(proxy.clone()),
        None => {
            state
                .resolve_transport_proxy_snapshot_with_tunnel_affinity(transport)
                .await
        }
    };
    let timeouts = state
        .resolve_transport_execution_timeouts(transport)
        .or(Some(default_provider_quota_execution_timeouts(
            proxy.as_ref(),
        )));
    let spec =
        build_chatgpt_web_pool_quota_request(&transport.key.id, &endpoint.base_url, authorization);
    let resolved_transport_profile = state.resolve_transport_profile(transport);
    let plan = super::shared::build_provider_quota_execution_plan(
        transport,
        spec,
        proxy,
        chatgpt_web_quota_transport_profile(resolved_transport_profile.as_ref()),
        timeouts,
    );

    execute_provider_quota_plan(state, transport, plan, "chatgpt_web").await
}

fn chatgpt_web_quota_transport_profile(
    transport_profile: Option<&ResolvedTransportProfile>,
) -> Option<ResolvedTransportProfile> {
    match transport_profile {
        Some(profile)
            if profile
                .backend
                .trim()
                .eq_ignore_ascii_case(TRANSPORT_BACKEND_BROWSER_WREQ) =>
        {
            Some(profile.clone())
        }
        _ => Some(default_chatgpt_web_quota_transport_profile()),
    }
}

fn default_chatgpt_web_quota_transport_profile() -> ResolvedTransportProfile {
    ResolvedTransportProfile {
        profile_id: CHATGPT_WEB_BROWSER_PROFILE.to_string(),
        backend: TRANSPORT_BACKEND_BROWSER_WREQ.to_string(),
        http_mode: TRANSPORT_HTTP_MODE_AUTO.to_string(),
        pool_scope: TRANSPORT_POOL_SCOPE_KEY.to_string(),
        header_fingerprint: None,
        extra: Some(json!({
            "browser_profile": CHATGPT_WEB_BROWSER_PROFILE,
            "source": "chatgpt_web_quota_default",
        })),
    }
}

fn chatgpt_web_quota_error_detail(result: &ExecutionResult) -> Option<String> {
    extract_execution_error_message(result).or_else(|| {
        let body = result.body.as_ref()?.body_bytes_b64.as_deref()?;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(body)
            .ok()?;
        let text = String::from_utf8_lossy(&decoded).trim().to_string();
        (!text.is_empty()).then_some(text)
    })
}

fn chatgpt_web_is_structured_account_block(message: &str) -> bool {
    let lowered = message.to_ascii_lowercase();
    [
        "account has been disabled",
        "account disabled",
        "account has been deactivated",
        "account_deactivated",
        "account deactivated",
        "organization has been disabled",
        "organization_disabled",
        "deactivated_workspace",
        "account suspended",
        "account banned",
        "account_block",
        "account blocked",
        "访问被禁止",
        "账户访问被禁止",
        "账户已封禁",
        "封禁",
        "封号",
        "被封",
    ]
    .iter()
    .any(|keyword| lowered.contains(keyword))
}

fn chatgpt_web_quota_403_refresh_failed_reason(message: Option<&str>) -> String {
    let detail = message
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .filter(|value| !value.contains('<'))
        .unwrap_or("ChatGPT Web 访问验证失败，请检查浏览器指纹、Cloudflare 验证或代理/地区限制");
    format!("{OAUTH_REFRESH_FAILED_PREFIX}{detail}")
}

fn chatgpt_web_quota_invalid_reason(status_code: u16, upstream_message: Option<&str>) -> String {
    let message = upstream_message.unwrap_or_default().trim();
    if status_code == 403 && !chatgpt_web_is_structured_account_block(message) {
        return chatgpt_web_quota_403_refresh_failed_reason(upstream_message);
    }
    let detail = if message.is_empty() {
        match status_code {
            401 => "ChatGPT Web Token 无效或已过期",
            403 => "ChatGPT Web 账户访问受限",
            _ => "ChatGPT Web 请求失败",
        }
    } else {
        message
    };
    match status_code {
        401 => format!("{OAUTH_EXPIRED_PREFIX}{detail}"),
        403 => format!("{OAUTH_ACCOUNT_BLOCK_PREFIX}{detail}"),
        _ => detail.to_string(),
    }
}

fn chatgpt_web_quota_result_message(reason: &str) -> String {
    for prefix in [
        OAUTH_REFRESH_FAILED_PREFIX,
        OAUTH_EXPIRED_PREFIX,
        OAUTH_ACCOUNT_BLOCK_PREFIX,
    ] {
        if let Some(message) = reason.strip_prefix(prefix) {
            return message.trim().to_string();
        }
    }
    reason.trim().to_string()
}

pub(crate) async fn refresh_chatgpt_web_provider_quota_locally(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    endpoint: &StoredProviderCatalogEndpoint,
    keys: Vec<StoredProviderCatalogKey>,
    proxy_override: Option<ProxySnapshot>,
) -> Result<Option<serde_json::Value>, GatewayError> {
    let mut results = Vec::new();
    let mut success_count = 0usize;
    let mut failed_count = 0usize;
    let mut auto_removed_count = 0usize;

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

        let authorization = match resolve_chatgpt_web_quota_auth(state, &transport).await? {
            Some(auth) => auth,
            None => {
                if quota_key_auto_removed(state, &key.id).await? {
                    auto_removed_count += 1;
                    results.push(oauth_refresh_auto_removed_result(&key));
                    continue;
                }
                failed_count += 1;
                results.push(json!({
                    "key_id": key.id,
                    "key_name": key.name,
                    "status": "error",
                    "message": "缺少 ChatGPT Web OAuth 认证信息，请先导入/刷新 Token",
                }));
                continue;
            }
        };

        let result = match execute_chatgpt_web_quota_plan(
            state,
            &transport,
            endpoint,
            authorization,
            proxy_override.as_ref(),
        )
        .await?
        {
            ProviderQuotaExecutionOutcome::Response(result) => result,
            ProviderQuotaExecutionOutcome::Failure(detail) => {
                failed_count += 1;
                results.push(json!({
                    "key_id": key.id,
                    "key_name": key.name,
                    "status": "error",
                    "message": format!("conversation/init 请求执行失败: {detail}"),
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
        let (mut oauth_invalid_at_unix_secs, mut oauth_invalid_reason) = (
            key.oauth_invalid_at_unix_secs,
            key.oauth_invalid_reason.clone(),
        );
        let mut status = "error".to_string();
        let mut message = None::<String>;

        if result.status_code == 200 {
            if let Some(body_json) = result
                .body
                .as_ref()
                .and_then(|body| body.json_body.as_ref())
            {
                if let Some(mut metadata) =
                    parse_chatgpt_web_conversation_init_response(body_json, now_unix_secs)
                {
                    let auth_config = chatgpt_web_auth_config(&transport);
                    enrich_chatgpt_web_quota_metadata(&mut metadata, auth_config.as_ref());
                    normalize_chatgpt_web_image_quota_limit(
                        &mut metadata,
                        key.upstream_metadata.as_ref(),
                    );
                    metadata_update = Some(json!({ "chatgpt_web": metadata }));
                    (oauth_invalid_at_unix_secs, oauth_invalid_reason) =
                        quota_refresh_success_invalid_state(&key);
                    status = "success".to_string();
                } else {
                    status = "no_metadata".to_string();
                    message = Some("响应中未包含 ChatGPT Web 生图限额信息".to_string());
                }
            } else {
                status = "no_metadata".to_string();
                message = Some("响应中未包含 ChatGPT Web 生图限额信息".to_string());
            }
        } else {
            let err_msg = chatgpt_web_quota_error_detail(&result);
            let invalid_reason = if matches!(result.status_code, 401 | 403) {
                Some(chatgpt_web_quota_invalid_reason(
                    result.status_code,
                    err_msg.as_deref(),
                ))
            } else {
                None
            };
            let display_detail = invalid_reason
                .as_deref()
                .map(chatgpt_web_quota_result_message)
                .or_else(|| err_msg.clone());
            message = Some(match display_detail.as_deref() {
                Some(detail) if !detail.is_empty() => {
                    format!(
                        "conversation/init 返回状态码 {}: {}",
                        result.status_code, detail
                    )
                }
                _ => format!("conversation/init 返回状态码 {}", result.status_code),
            });

            if matches!(result.status_code, 401 | 403) {
                oauth_invalid_at_unix_secs = Some(now_unix_secs);
                oauth_invalid_reason = invalid_reason;
                status = if result.status_code == 401 {
                    "auth_invalid".to_string()
                } else if oauth_invalid_reason
                    .as_deref()
                    .is_some_and(|reason| reason.starts_with(OAUTH_REFRESH_FAILED_PREFIX))
                {
                    "refresh_failed".to_string()
                } else {
                    "forbidden".to_string()
                };
            }
        }

        if !persist_provider_quota_refresh_state(
            state,
            &key.id,
            metadata_update.as_ref(),
            oauth_invalid_at_unix_secs,
            oauth_invalid_reason,
            None,
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
        if result.status_code != 200 {
            payload.insert("status_code".to_string(), json!(result.status_code));
        }
        if let Some(metadata) = metadata_update
            .as_ref()
            .and_then(|value| value.get("chatgpt_web"))
            .cloned()
        {
            payload.insert("metadata".to_string(), metadata);
        }
        if let Some(quota_snapshot) = build_quota_snapshot_payload(
            "chatgpt_web",
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
        "total": results.len(),
        "results": results,
        "message": format!("已处理 {} 个 Key", results.len()),
        "auto_removed": auto_removed_count,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_contracts::{ResponseBody, TRANSPORT_BACKEND_REQWEST_RUSTLS};
    use base64::Engine as _;
    use std::collections::BTreeMap;

    #[test]
    fn quota_refresh_defaults_to_browser_wreq_transport() {
        let profile = chatgpt_web_quota_transport_profile(None).expect("transport profile");

        assert_eq!(profile.backend, TRANSPORT_BACKEND_BROWSER_WREQ);
        assert_eq!(profile.profile_id, CHATGPT_WEB_BROWSER_PROFILE);
        assert_eq!(profile.http_mode, TRANSPORT_HTTP_MODE_AUTO);
        assert_eq!(profile.pool_scope, TRANSPORT_POOL_SCOPE_KEY);
        assert_eq!(
            profile
                .extra
                .as_ref()
                .and_then(|value| value.get("browser_profile"))
                .and_then(serde_json::Value::as_str),
            Some(CHATGPT_WEB_BROWSER_PROFILE)
        );
    }

    #[test]
    fn quota_refresh_overrides_non_browser_transport() {
        let reqwest_profile = ResolvedTransportProfile {
            profile_id: "chrome_136".to_string(),
            backend: TRANSPORT_BACKEND_REQWEST_RUSTLS.to_string(),
            http_mode: TRANSPORT_HTTP_MODE_AUTO.to_string(),
            pool_scope: TRANSPORT_POOL_SCOPE_KEY.to_string(),
            header_fingerprint: None,
            extra: None,
        };

        let profile =
            chatgpt_web_quota_transport_profile(Some(&reqwest_profile)).expect("transport profile");

        assert_eq!(profile.backend, TRANSPORT_BACKEND_BROWSER_WREQ);
        assert_eq!(profile.profile_id, CHATGPT_WEB_BROWSER_PROFILE);
    }

    #[test]
    fn browser_challenge_403_is_not_account_block() {
        let body = "<!DOCTYPE html><html><head><title>Just a moment...</title></head><body>Cloudflare</body></html>";
        let result = ExecutionResult {
            request_id: "chatgpt-web-quota:test".to_string(),
            candidate_id: None,
            status_code: 403,
            headers: BTreeMap::new(),
            body: Some(ResponseBody {
                json_body: None,
                body_bytes_b64: Some(base64::engine::general_purpose::STANDARD.encode(body)),
            }),
            telemetry: None,
            error: None,
        };

        let detail = chatgpt_web_quota_error_detail(&result).expect("html body should decode");
        let reason = chatgpt_web_quota_invalid_reason(result.status_code, Some(&detail));

        assert!(reason.starts_with(OAUTH_REFRESH_FAILED_PREFIX));
        assert!(!reason.starts_with(OAUTH_ACCOUNT_BLOCK_PREFIX));
        assert_eq!(
            chatgpt_web_quota_result_message(&reason),
            "ChatGPT Web 访问验证失败，请检查浏览器指纹、Cloudflare 验证或代理/地区限制"
        );
    }

    #[test]
    fn explicit_account_block_403_remains_account_block() {
        let reason = chatgpt_web_quota_invalid_reason(403, Some("account has been deactivated"));

        assert!(reason.starts_with(OAUTH_ACCOUNT_BLOCK_PREFIX));
    }
}
