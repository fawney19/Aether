use super::shared::{
    build_quota_snapshot_payload, execute_provider_quota_plan, extract_execution_error_message_ref,
    persist_provider_quota_refresh_state, quota_refresh_success_invalid_state,
    resolve_provider_quota_execution_timeouts, ProviderQuotaExecutionOutcome,
};
use crate::execution_runtime::transport::decode_base64_body_with_limit;
use crate::handlers::admin::provider::shared::payloads::{
    OAUTH_ACCOUNT_BLOCK_PREFIX, OAUTH_EXPIRED_PREFIX, OAUTH_REFRESH_FAILED_PREFIX,
};
use crate::handlers::admin::request::{AdminAppState, AdminGatewayProviderTransportSnapshot};
use crate::GatewayError;
use aether_admin::provider::redaction::admin_provider_metadata_bucket_safe_json;
use aether_contracts::{
    ExecutionPlan, ExecutionResult, ProxySnapshot, RequestBody, ResolvedTransportProfile,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_provider_pool::{
    grok_pool_tier_from_quota_bucket, grok_supported_quota_windows_for_tier,
};
use aether_provider_transport::grok_browser_profile_metadata_from_resolved_transport_profile;
use serde_json::json;
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const GROK_DEFAULT_BASE_URL: &str = "https://grok.com";
const GROK_RATE_LIMITS_PATH: &str = "/rest/rate-limits";
const GROK_STATSIG_ID: &str = "ZTpUeXBlRXJyb3I6IENhbm5vdCByZWFkIHByb3BlcnRpZXMgb2YgdW5kZWZpbmVkIChyZWFkaW5nICdjaGlsZE5vZGVzJyk=";

fn grok_base_url(endpoint: &StoredProviderCatalogEndpoint) -> String {
    let base_url = endpoint.base_url.trim().trim_end_matches('/');
    if base_url.is_empty() {
        GROK_DEFAULT_BASE_URL.to_string()
    } else {
        base_url.to_string()
    }
}

fn grok_auth_config(
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

fn grok_auth_string(auth_config: Option<&serde_json::Value>, fields: &[&str]) -> Option<String> {
    let object = auth_config.and_then(serde_json::Value::as_object)?;
    fields.iter().find_map(|field| {
        object
            .get(*field)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn build_grok_quota_headers(
    auth_config: Option<&serde_json::Value>,
    transport_profile: Option<&ResolvedTransportProfile>,
    base_url: &str,
) -> Option<BTreeMap<String, String>> {
    let cookie = build_grok_quota_cookie(auth_config).unwrap_or_default();
    let browser_profile =
        grok_browser_profile_metadata_from_resolved_transport_profile(transport_profile?)?;
    Some(BTreeMap::from([
        ("accept".to_string(), "*/*".to_string()),
        (
            "accept-language".to_string(),
            "zh-CN,zh;q=0.9,en;q=0.8".to_string(),
        ),
        (
            "baggage".to_string(),
            "sentry-environment=production,sentry-release=d6add6fb0460641fd482d767a335ef72b9b6abb8,sentry-public_key=b311e0f2690c81f25e2c4cf6d4f7ce1c".to_string(),
        ),
        ("content-type".to_string(), "application/json".to_string()),
        ("origin".to_string(), base_url.to_string()),
        ("priority".to_string(), "u=1, i".to_string()),
        ("referer".to_string(), format!("{base_url}/")),
        ("sec-ch-ua".to_string(), browser_profile.sec_ch_ua),
        ("sec-ch-ua-mobile".to_string(), "?0".to_string()),
        ("sec-ch-ua-model".to_string(), String::new()),
        (
            "sec-ch-ua-platform".to_string(),
            browser_profile.sec_ch_ua_platform,
        ),
        ("sec-fetch-dest".to_string(), "empty".to_string()),
        ("sec-fetch-mode".to_string(), "cors".to_string()),
        ("sec-fetch-site".to_string(), "same-origin".to_string()),
        ("user-agent".to_string(), browser_profile.user_agent),
        ("cookie".to_string(), cookie),
        ("x-statsig-id".to_string(), GROK_STATSIG_ID.to_string()),
        ("x-xai-request-id".to_string(), Uuid::new_v4().to_string()),
    ]))
}

fn build_grok_quota_cookie(auth_config: Option<&serde_json::Value>) -> Option<String> {
    let token = grok_auth_string(auth_config, &["sso_token", "access_token", "token"])?;
    let token = strip_cookie_prefix(token.trim(), "sso=");
    if token.is_empty() {
        return None;
    }
    let sso_rw = grok_auth_string(auth_config, &["sso_rw_token", "ssoRwToken"])
        .map(|value| strip_cookie_prefix(value.trim(), "sso-rw="))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| token.clone());

    let mut parts = vec![format!("sso={token}"), format!("sso-rw={sso_rw}")];
    if let Some(extra_cookies) =
        grok_auth_string(auth_config, &["cf_cookies", "cfCookies", "cookie"])
            .and_then(|value| normalize_grok_extra_cookies(value.as_str()))
    {
        parts.push(extra_cookies);
    }
    let cf_clearance = grok_auth_string(auth_config, &["cf_clearance", "cfClearance"])
        .map(|value| strip_cookie_prefix(value.trim(), "cf_clearance="))
        .filter(|value| !value.is_empty());
    if let Some(cf_clearance) = cf_clearance {
        if !parts.iter().any(|part| part.contains("cf_clearance=")) {
            parts.push(format!("cf_clearance={cf_clearance}"));
        }
    }
    Some(parts.join("; "))
}

fn strip_cookie_prefix(value: &str, prefix: &str) -> String {
    value
        .strip_prefix(prefix)
        .map(str::trim)
        .unwrap_or(value)
        .to_string()
}

fn normalize_grok_extra_cookies(value: &str) -> Option<String> {
    let parts = value
        .trim()
        .trim_matches(';')
        .split(';')
        .filter_map(|segment| {
            let (name, value) = segment.trim().split_once('=')?;
            let name = name.trim();
            let value = value.trim();
            if name.is_empty()
                || value.is_empty()
                || name.eq_ignore_ascii_case("sso")
                || name.eq_ignore_ascii_case("sso-rw")
            {
                return None;
            }
            Some(format!("{name}={value}"))
        })
        .collect::<Vec<_>>();
    (!parts.is_empty()).then(|| parts.join("; "))
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct GrokRateLimitSnapshot {
    remaining: f64,
    total: f64,
    window_seconds: u64,
    wait_time_seconds: Option<u64>,
}

impl GrokRateLimitSnapshot {
    fn reset_after_seconds(self) -> u64 {
        self.wait_time_seconds.unwrap_or(self.window_seconds)
    }

    fn reset_at_source(self) -> &'static str {
        if self.wait_time_seconds.is_some() {
            "grok_rate_limits_wait_time"
        } else {
            "grok_rate_limits_window"
        }
    }
}

fn parse_grok_rate_limits(body: &serde_json::Value) -> Option<GrokRateLimitSnapshot> {
    let remaining = body
        .get("remainingQueries")
        .and_then(serde_json::Value::as_f64)?;
    let total = body
        .get("totalQueries")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(remaining.max(0.0));
    let window_seconds = body
        .get("windowSizeSeconds")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(72_000);
    let wait_time_seconds = body
        .get("waitTimeSeconds")
        .and_then(serde_json::Value::as_u64);
    Some(GrokRateLimitSnapshot {
        remaining,
        total,
        window_seconds,
        wait_time_seconds,
    })
}

fn grok_pool_tier_hint_for_refresh(
    key: &StoredProviderCatalogKey,
    auth_config: Option<&serde_json::Value>,
) -> Option<&'static str> {
    key.status_snapshot
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|snapshot| snapshot.get("quota"))
        .and_then(serde_json::Value::as_object)
        .and_then(grok_pool_tier_from_quota_bucket)
        .or_else(|| {
            key.upstream_metadata
                .as_ref()
                .and_then(serde_json::Value::as_object)
                .and_then(|metadata| metadata.get("grok"))
                .and_then(serde_json::Value::as_object)
                .and_then(grok_pool_tier_from_quota_bucket)
        })
        .or_else(|| {
            auth_config
                .and_then(serde_json::Value::as_object)
                .and_then(grok_pool_tier_from_quota_bucket)
        })
}

async fn execute_grok_quota_plan(
    state: &AdminAppState<'_>,
    transport: &AdminGatewayProviderTransportSnapshot,
    endpoint: &StoredProviderCatalogEndpoint,
    body: serde_json::Value,
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
    let timeouts = Some(resolve_provider_quota_execution_timeouts(
        state.resolve_transport_execution_timeouts(transport),
        proxy.as_ref(),
    ));
    let transport_profile = state.resolve_transport_profile(transport);
    let base_url = grok_base_url(endpoint);
    let headers = build_grok_quota_headers(
        grok_auth_config(transport).as_ref(),
        transport_profile.as_ref(),
        &base_url,
    )
    .ok_or_else(|| {
        GatewayError::Internal("unsupported Grok browser transport profile".to_string())
    })?;
    let plan = ExecutionPlan {
        request_id: format!("grok-quota:{}", transport.key.id),
        candidate_id: None,
        provider_name: Some("grok".to_string()),
        provider_id: transport.provider.id.clone(),
        endpoint_id: transport.endpoint.id.clone(),
        key_id: transport.key.id.clone(),
        method: "POST".to_string(),
        url: format!(
            "{}/{}",
            base_url,
            GROK_RATE_LIMITS_PATH.trim_start_matches('/')
        ),
        headers,
        content_type: Some("application/json".to_string()),
        content_encoding: None,
        body: RequestBody::from_json(body),
        stream: false,
        client_api_format: "openai:responses".to_string(),
        provider_api_format: "grok:rate_limits".to_string(),
        model_name: Some("grok-quota".to_string()),
        proxy,
        transport_profile,
        timeouts,
    };

    execute_provider_quota_plan(state, transport, plan, "grok").await
}

fn grok_quota_error_detail(result: &ExecutionResult) -> Option<String> {
    extract_execution_error_message_ref(result)
        .map(bound_quota_error_detail)
        .or_else(|| {
            let body = result.body.as_ref()?.body_bytes_b64.as_deref()?;
            let decoded = decode_base64_body_with_limit(body, crate::MAX_ERROR_BODY_BYTES).ok()?;
            let text = String::from_utf8_lossy(&decoded);
            let text = text.trim();
            (!text.is_empty()).then(|| bound_quota_error_detail(text))
        })
}

fn bound_quota_error_detail(value: &str) -> String {
    let value = value.trim();
    let end = value.floor_char_boundary(value.len().min(crate::MAX_ERROR_BODY_BYTES));
    value[..end].to_string()
}

fn grok_is_cloudflare_challenge(message: &str) -> bool {
    let lowered = message.to_ascii_lowercase();
    lowered.contains("cloudflare")
        || lowered.contains("just a moment")
        || lowered.contains("__cf_chl")
        || lowered.contains("cf-ray")
}

fn grok_quota_invalid_reason(status_code: u16, upstream_message: Option<&str>) -> String {
    let message = upstream_message.unwrap_or_default().trim();
    if status_code == 403 && grok_is_cloudflare_challenge(message) {
        return format!(
            "{OAUTH_REFRESH_FAILED_PREFIX}Grok Cloudflare 验证失败，请重新从同一浏览器复制最新 Cookie 和 User-Agent，或配置可通过 Cloudflare 的代理运行时"
        );
    }
    let detail = match status_code {
        401 => "Grok Token 无效或已过期",
        403 => "Grok 账户访问受限",
        _ => "Grok 请求失败",
    };
    match status_code {
        401 => format!("{OAUTH_EXPIRED_PREFIX}{detail}"),
        403 => format!("{OAUTH_ACCOUNT_BLOCK_PREFIX}{detail}"),
        _ => detail.to_string(),
    }
}

fn grok_quota_result_message(reason: &str) -> String {
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

pub(crate) async fn refresh_grok_provider_quota_locally(
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

        if grok_auth_config(&transport).is_none() {
            failed_count += 1;
            results.push(json!({
                "key_id": key.id,
                "key_name": key.name,
                "status": "error",
                "message": "缺少 Grok 账号会话信息，请先导入 Token",
            }));
            continue;
        }

        let auth_config = grok_auth_config(&transport);
        let mut quota_by_model = serde_json::Map::new();
        let mut refreshed = false;
        let mut invalid_reason = None::<String>;
        let mut invalid_at = key.oauth_invalid_at_unix_secs;
        let mut last_status_code = None::<u16>;
        let mut last_error_message = None::<String>;
        let mut metadata_update = serde_json::Map::new();
        let base_url = grok_base_url(endpoint);

        let supported_windows = grok_supported_quota_windows_for_tier(
            grok_pool_tier_hint_for_refresh(&key, auth_config.as_ref()),
        );
        for (quota_key, mode_name) in supported_windows.iter().copied() {
            let result = match execute_grok_quota_plan(
                state,
                &transport,
                endpoint,
                json!({ "modelName": mode_name }),
                proxy_override.as_ref(),
            )
            .await?
            {
                ProviderQuotaExecutionOutcome::Response(result) => result,
                ProviderQuotaExecutionOutcome::Failure(_) => {
                    last_error_message = Some("rate-limits 请求执行失败".to_string());
                    continue;
                }
            };
            last_status_code = Some(result.status_code);

            if result.status_code == 200 {
                if let Some(body_json) = result
                    .body
                    .as_ref()
                    .and_then(|body| body.json_body.as_ref())
                {
                    if let Some(rate_limit) = parse_grok_rate_limits(body_json) {
                        refreshed = true;
                        let now_unix_secs = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .ok()
                            .map(|duration| duration.as_secs())
                            .unwrap_or(0);
                        let reset_after_seconds = rate_limit.reset_after_seconds();
                        let reset_at = now_unix_secs.saturating_add(reset_after_seconds);
                        quota_by_model.insert(
                            (*quota_key).to_string(),
                            json!({
                                "display_name": *mode_name,
                                "remaining_fraction": if rate_limit.total > 0.0 { Some((rate_limit.remaining / rate_limit.total).clamp(0.0, 1.0)) } else { None::<f64> },
                                "used_percent": if rate_limit.total > 0.0 { Some(((rate_limit.total - rate_limit.remaining).max(0.0) / rate_limit.total * 100.0).clamp(0.0, 100.0)) } else { None::<f64> },
                                "remaining": rate_limit.remaining,
                                "total": rate_limit.total,
                                "window_seconds": rate_limit.window_seconds,
                                "wait_time_seconds": rate_limit.wait_time_seconds,
                                "reset_after_seconds": reset_after_seconds,
                                "reset_at": reset_at,
                                "next_reset_at": reset_at,
                                "reset_at_source": rate_limit.reset_at_source(),
                                "is_exhausted": rate_limit.remaining <= 0.0,
                            }),
                        );
                    } else {
                        last_error_message = Some(
                            "Grok rate-limits 未返回 remainingQueries/totalQueries".to_string(),
                        );
                    }
                } else {
                    last_error_message = Some("Grok rate-limits 未返回 JSON 数据".to_string());
                }
            } else if matches!(result.status_code, 401 | 403) {
                let now_unix_secs = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .ok()
                    .map(|duration| duration.as_secs())
                    .unwrap_or(0);
                invalid_at = Some(now_unix_secs);
                let error_detail = grok_quota_error_detail(&result);
                invalid_reason = Some(grok_quota_invalid_reason(
                    result.status_code,
                    error_detail.as_deref(),
                ));
                last_error_message = invalid_reason.as_deref().map(grok_quota_result_message);
            } else {
                last_error_message = Some(format!(
                    "Grok rate-limits 请求失败 ({})",
                    result.status_code
                ));
            }
        }

        if refreshed {
            if let Some(pool_tier) = grok_pool_tier_from_quota_bucket(&quota_by_model)
                .or_else(|| grok_pool_tier_hint_for_refresh(&key, auth_config.as_ref()))
            {
                let pool_tier_value = json!(pool_tier);
                metadata_update.insert("pool_tier".to_string(), pool_tier_value.clone());
                metadata_update
                    .entry("plan_type".to_string())
                    .or_insert(pool_tier_value);
            }
            metadata_update.insert(
                "updated_at".to_string(),
                json!(SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .ok()
                    .map(|duration| duration.as_secs())
                    .unwrap_or(0)),
            );
            metadata_update.insert("base_url".to_string(), json!(base_url));
            metadata_update.insert("quota_by_model".to_string(), json!(quota_by_model));
        }

        let metadata_update_value = if metadata_update.is_empty() {
            None
        } else {
            Some(serde_json::Value::Object({
                let mut map = serde_json::Map::new();
                map.insert(
                    "grok".to_string(),
                    serde_json::Value::Object(metadata_update.clone()),
                );
                map
            }))
        };

        if !persist_provider_quota_refresh_state(
            state,
            &key.id,
            metadata_update_value.as_ref(),
            invalid_at,
            invalid_reason,
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

        if refreshed {
            success_count += 1;
        } else {
            failed_count += 1;
        }

        let mut payload = serde_json::Map::new();
        payload.insert("key_id".to_string(), json!(key.id));
        payload.insert("key_name".to_string(), json!(key.name));
        payload.insert(
            "status".to_string(),
            json!(if refreshed { "success" } else { "error" }),
        );
        if let Some(metadata) = metadata_update.get("quota_by_model") {
            payload.insert(
                "metadata".to_string(),
                admin_provider_metadata_bucket_safe_json("grok", Some(metadata)),
            );
        }
        if let Some(quota_snapshot) = build_quota_snapshot_payload(
            "grok",
            key.status_snapshot.as_ref(),
            metadata_update_value.as_ref(),
        ) {
            payload.insert("quota_snapshot".to_string(), quota_snapshot);
        }
        if !refreshed {
            payload.insert(
                "message".to_string(),
                json!(last_error_message.unwrap_or_else(|| {
                    "Grok rate-limits 未返回可用配额数据".to_string()
                })),
            );
            if let Some(status_code) = last_status_code {
                payload.insert("status_code".to_string(), json!(status_code));
            }
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
    use super::{
        build_grok_quota_cookie, build_grok_quota_headers, grok_pool_tier_hint_for_refresh,
        grok_quota_error_detail, grok_quota_invalid_reason, grok_quota_result_message,
        parse_grok_rate_limits,
    };
    use crate::handlers::admin::provider::shared::payloads::OAUTH_REFRESH_FAILED_PREFIX;
    use aether_contracts::{ExecutionResult, ResponseBody};
    use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
    use base64::Engine as _;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn sample_key(
        status_snapshot: Option<serde_json::Value>,
        upstream_metadata: Option<serde_json::Value>,
    ) -> StoredProviderCatalogKey {
        let mut key = StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-1".to_string(),
            "key-1".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build");
        key.status_snapshot = status_snapshot;
        key.upstream_metadata = upstream_metadata;
        key
    }

    #[test]
    fn quota_cookie_preserves_grok_session_and_clearance() {
        let auth_config = json!({
            "sso_token": "sso=abc",
            "sso_rw_token": "sso-rw=rw",
            "cf_clearance": "cf"
        });

        let cookie = build_grok_quota_cookie(Some(&auth_config)).expect("cookie should build");

        assert_eq!(cookie, "sso=abc; sso-rw=rw; cf_clearance=cf");
    }

    #[test]
    fn quota_cookie_removes_duplicate_session_cookies_from_cf_profile() {
        let auth_config = json!({
            "sso_token": "abc",
            "sso_rw_token": "rw",
            "cf_cookies": "i18nextLng=zh; sso=ignored; sso-rw=ignored-rw; cf_clearance=cf"
        });

        let cookie = build_grok_quota_cookie(Some(&auth_config)).expect("cookie should build");

        assert_eq!(cookie, "sso=abc; sso-rw=rw; i18nextLng=zh; cf_clearance=cf");
    }

    #[test]
    fn quota_headers_use_resolved_transport_profile_user_agent() {
        let auth_config = json!({
            "sso_token": "abc",
            "user_agent": "Mozilla/5.0 custom"
        });
        let transport_profile = aether_provider_transport::grok_browser_resolved_transport_profile(
            Some("chrome137"),
            "test",
        )
        .expect("profile should resolve");

        let headers = build_grok_quota_headers(
            Some(&auth_config),
            Some(&transport_profile),
            "https://grok.com",
        )
        .expect("headers should build");

        assert!(headers
            .get("user-agent")
            .is_some_and(|value| value.contains("Chrome/137.0.0.0")));
        assert_eq!(
            headers.get("sec-ch-ua"),
            Some(
                &r#""Google Chrome";v="137", "Chromium";v="137", "Not(A:Brand";v="24""#.to_string()
            )
        );
    }

    #[test]
    fn quota_headers_default_to_chrome136_clearance_profile() {
        let auth_config = json!({
            "sso_token": "abc"
        });

        let transport_profile =
            aether_provider_transport::grok_browser_resolved_transport_profile(None, "test")
                .expect("profile should resolve");
        let headers = build_grok_quota_headers(
            Some(&auth_config),
            Some(&transport_profile),
            "https://grok.com",
        )
        .expect("headers should build");

        assert!(headers
            .get("user-agent")
            .is_some_and(|value| value.contains("Chrome/136.0.0.0")));
        assert_eq!(
            headers.get("sec-ch-ua"),
            Some(
                &r#""Google Chrome";v="136", "Chromium";v="136", "Not(A:Brand";v="24""#.to_string()
            )
        );
        assert_eq!(
            headers.get("sec-ch-ua-platform"),
            Some(&r#""macOS""#.to_string())
        );
        assert!(headers.contains_key("x-statsig-id"));
        assert!(headers.contains_key("x-xai-request-id"));
    }

    #[test]
    fn quota_headers_do_not_mark_rate_limits_as_grok_app_chat_runtime() {
        let auth_config = json!({
            "sso_token": "abc"
        });

        let transport_profile =
            aether_provider_transport::grok_browser_resolved_transport_profile(None, "test")
                .expect("profile should resolve");
        let headers = build_grok_quota_headers(
            Some(&auth_config),
            Some(&transport_profile),
            "https://grok.com",
        )
        .expect("headers should build");

        assert!(!headers.contains_key(aether_provider_transport::GROK_INTERNAL_HEADER));
    }

    #[test]
    fn parses_grok_wait_time_seconds_as_authoritative_reset_delay() {
        let body = json!({
            "windowSizeSeconds": 86_400,
            "remainingQueries": 0,
            "waitTimeSeconds": 12_648,
            "totalQueries": 30,
            "lowEffortRateLimits": null,
            "highEffortRateLimits": null
        });

        let rate_limits = parse_grok_rate_limits(&body).expect("rate limits should parse");

        assert_eq!(rate_limits.remaining, 0.0);
        assert_eq!(rate_limits.total, 30.0);
        assert_eq!(rate_limits.window_seconds, 86_400);
        assert_eq!(rate_limits.wait_time_seconds, Some(12_648));
        assert_eq!(rate_limits.reset_after_seconds(), 12_648);
        assert_eq!(rate_limits.reset_at_source(), "grok_rate_limits_wait_time");
    }

    #[test]
    fn parses_grok_rate_limits_falls_back_to_window_when_wait_time_is_absent() {
        let body = json!({
            "windowSizeSeconds": 86_400,
            "remainingQueries": 12,
            "totalQueries": 30
        });

        let rate_limits = parse_grok_rate_limits(&body).expect("rate limits should parse");

        assert_eq!(rate_limits.remaining, 12.0);
        assert_eq!(rate_limits.total, 30.0);
        assert_eq!(rate_limits.window_seconds, 86_400);
        assert_eq!(rate_limits.wait_time_seconds, None);
        assert_eq!(rate_limits.reset_after_seconds(), 86_400);
        assert_eq!(rate_limits.reset_at_source(), "grok_rate_limits_window");
    }

    #[test]
    fn infers_grok_pool_tier_from_live_quota_totals() {
        let key = sample_key(
            Some(json!({
                "quota": {
                    "pool_tier": "heavy"
                }
            })),
            None,
        );

        assert_eq!(
            grok_pool_tier_hint_for_refresh(&key, Some(&json!({}))),
            Some("heavy")
        );
    }

    #[test]
    fn infers_basic_grok_pool_tier_from_fast_quota_when_auto_is_absent() {
        let key = sample_key(
            None,
            Some(json!({
                "grok": {
                    "plan_type": "basic"
                }
            })),
        );

        assert_eq!(grok_pool_tier_hint_for_refresh(&key, None), Some("basic"));
    }

    #[test]
    fn cloudflare_challenge_403_is_not_account_block() {
        let body = "<!DOCTYPE html><html><head><title>Just a moment...</title></head><body>Cloudflare</body></html>";
        let result = ExecutionResult {
            request_id: "grok-quota:test".to_string(),
            candidate_id: None,
            status_code: 403,
            headers: BTreeMap::new(),
            response_observation: None,
            body: Some(ResponseBody {
                json_body: None,
                body_bytes_b64: Some(base64::engine::general_purpose::STANDARD.encode(body)),
            }),
            telemetry: None,
            error: None,
        };

        let detail = grok_quota_error_detail(&result).expect("html body should be decoded");
        let reason = grok_quota_invalid_reason(result.status_code, Some(&detail));

        assert!(reason.starts_with("[REFRESH_FAILED] "));
        assert!(!reason.starts_with("[ACCOUNT_BLOCK] "));
        assert!(reason.contains("Cloudflare"));
    }

    #[test]
    fn quota_error_detail_is_bounded_before_copying_json_message() {
        let message = format!("{}界", "x".repeat(crate::MAX_ERROR_BODY_BYTES));
        let result = ExecutionResult {
            request_id: "grok-quota:oversized-json".to_string(),
            candidate_id: None,
            status_code: 500,
            headers: BTreeMap::new(),
            response_observation: None,
            body: Some(ResponseBody {
                json_body: Some(json!({"error": {"message": message}})),
                body_bytes_b64: None,
            }),
            telemetry: None,
            error: None,
        };

        let detail = grok_quota_error_detail(&result).expect("JSON error detail");

        assert_eq!(detail.len(), crate::MAX_ERROR_BODY_BYTES);
        assert!(detail.bytes().all(|byte| byte == b'x'));
    }

    #[test]
    fn quota_error_detail_rejects_oversized_base64_before_decode() {
        let encoded_limit =
            crate::execution_runtime::transport::maximum_base64_len_for_decoded_limit(
                crate::MAX_ERROR_BODY_BYTES,
            );
        let result = ExecutionResult {
            request_id: "grok-quota:oversized-base64".to_string(),
            candidate_id: None,
            status_code: 500,
            headers: BTreeMap::new(),
            response_observation: None,
            body: Some(ResponseBody {
                json_body: None,
                body_bytes_b64: Some("A".repeat(encoded_limit + 1)),
            }),
            telemetry: None,
            error: None,
        };

        assert_eq!(grok_quota_error_detail(&result), None);
    }

    #[test]
    fn quota_result_message_removes_status_prefix() {
        let reason = format!("{OAUTH_REFRESH_FAILED_PREFIX}Grok Cloudflare 验证失败");

        assert_eq!(
            grok_quota_result_message(&reason),
            "Grok Cloudflare 验证失败"
        );
    }

    #[test]
    fn quota_invalid_reason_does_not_persist_upstream_credentials() {
        let reason = grok_quota_invalid_reason(
            401,
            Some("authorization=Bearer upstream-secret https://user:pass@example.test?q=secret"),
        );

        assert_eq!(reason, "[OAUTH_EXPIRED] Grok Token 无效或已过期");
        assert!(!reason.contains("upstream-secret"));
        assert!(!reason.contains("user:pass"));
    }
}
