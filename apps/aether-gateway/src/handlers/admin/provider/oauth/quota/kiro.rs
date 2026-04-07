use super::shared::{
    coerce_json_f64, execute_provider_quota_plan, extract_execution_error_message,
    persist_provider_quota_refresh_state, quota_refresh_success_invalid_state,
    ProviderQuotaExecutionOutcome,
};
use crate::handlers::admin::provider::shared::payloads::{
    KIRO_USAGE_LIMITS_PATH, KIRO_USAGE_SDK_VERSION,
};
use crate::handlers::admin::shared::encrypt_catalog_secret_with_fallbacks;
use crate::{AppState, GatewayError};
use aether_contracts::{ExecutionPlan, ExecutionTimeouts, RequestBody};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};
use url::form_urlencoded;
use uuid::Uuid;

fn compute_kiro_total_usage_limit(breakdown: &serde_json::Value) -> f64 {
    let mut total = breakdown
        .get("usageLimitWithPrecision")
        .and_then(coerce_json_f64)
        .unwrap_or(0.0);

    if breakdown
        .get("freeTrialInfo")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|free_trial| {
            free_trial
                .get("freeTrialStatus")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .is_some_and(|value| value.eq_ignore_ascii_case("ACTIVE"))
        })
    {
        total += breakdown
            .get("freeTrialInfo")
            .and_then(|value| value.get("usageLimitWithPrecision"))
            .and_then(coerce_json_f64)
            .unwrap_or(0.0);
    }

    if let Some(bonuses) = breakdown
        .get("bonuses")
        .and_then(serde_json::Value::as_array)
    {
        for bonus in bonuses {
            let is_active = bonus
                .get("status")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .is_some_and(|value| value.eq_ignore_ascii_case("ACTIVE"));
            if is_active {
                total += bonus
                    .get("usageLimit")
                    .and_then(coerce_json_f64)
                    .unwrap_or(0.0);
            }
        }
    }

    total
}

fn compute_kiro_current_usage(breakdown: &serde_json::Value) -> f64 {
    let mut total = breakdown
        .get("currentUsageWithPrecision")
        .and_then(coerce_json_f64)
        .unwrap_or(0.0);

    if breakdown
        .get("freeTrialInfo")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|free_trial| {
            free_trial
                .get("freeTrialStatus")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .is_some_and(|value| value.eq_ignore_ascii_case("ACTIVE"))
        })
    {
        total += breakdown
            .get("freeTrialInfo")
            .and_then(|value| value.get("currentUsageWithPrecision"))
            .and_then(coerce_json_f64)
            .unwrap_or(0.0);
    }

    if let Some(bonuses) = breakdown
        .get("bonuses")
        .and_then(serde_json::Value::as_array)
    {
        for bonus in bonuses {
            let is_active = bonus
                .get("status")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .is_some_and(|value| value.eq_ignore_ascii_case("ACTIVE"));
            if is_active {
                total += bonus
                    .get("currentUsage")
                    .and_then(coerce_json_f64)
                    .unwrap_or(0.0);
            }
        }
    }

    total
}

fn parse_kiro_usage_response(
    value: &serde_json::Value,
    updated_at_unix_secs: u64,
) -> Option<serde_json::Value> {
    let root = value.as_object()?;
    let breakdown = root
        .get("usageBreakdownList")
        .and_then(serde_json::Value::as_array)
        .and_then(|items| items.first())?;

    let usage_limit = compute_kiro_total_usage_limit(breakdown);
    let current_usage = compute_kiro_current_usage(breakdown);
    let remaining = (usage_limit - current_usage).max(0.0);
    let usage_percentage = if usage_limit > 0.0 {
        ((current_usage / usage_limit) * 100.0).min(100.0)
    } else {
        0.0
    };

    let mut result = serde_json::Map::new();
    result.insert("current_usage".to_string(), json!(current_usage));
    result.insert("usage_limit".to_string(), json!(usage_limit));
    result.insert("remaining".to_string(), json!(remaining));
    result.insert("usage_percentage".to_string(), json!(usage_percentage));
    result.insert("updated_at".to_string(), json!(updated_at_unix_secs));

    if let Some(subscription_title) = root
        .get("subscriptionInfo")
        .and_then(|value| value.get("subscriptionTitle"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        result.insert("subscription_title".to_string(), json!(subscription_title));
    }

    if let Some(next_reset_at) = root
        .get("nextDateReset")
        .and_then(coerce_json_f64)
        .or_else(|| breakdown.get("nextDateReset").and_then(coerce_json_f64))
    {
        result.insert("next_reset_at".to_string(), json!(next_reset_at));
    }

    let email = root
        .get("desktopUserInfo")
        .and_then(|value| value.get("email"))
        .or_else(|| root.get("userInfo").and_then(|value| value.get("email")))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(email) = email {
        result.insert("email".to_string(), json!(email));
    }

    Some(serde_json::Value::Object(result))
}

fn build_kiro_usage_headers(
    auth: &crate::provider_transport::kiro::KiroRequestAuth,
) -> BTreeMap<String, String> {
    let kiro_version = auth.auth_config.effective_kiro_version();
    let machine_id = auth.machine_id.trim();
    let ide_tag = if machine_id.is_empty() {
        format!("KiroIDE-{kiro_version}")
    } else {
        format!("KiroIDE-{kiro_version}-{machine_id}")
    };
    let host = format!(
        "q.{}.amazonaws.com",
        auth.auth_config.effective_api_region()
    );

    BTreeMap::from([
        (
            "x-amz-user-agent".to_string(),
            format!("aws-sdk-js/{KIRO_USAGE_SDK_VERSION} {ide_tag}"),
        ),
        (
            "user-agent".to_string(),
            format!(
                "aws-sdk-js/{KIRO_USAGE_SDK_VERSION} ua/2.1 os/other#unknown lang/js md/nodejs#22.21.1 api/codewhispererruntime#1.0.0 m/N,E {ide_tag}"
            ),
        ),
        ("host".to_string(), host),
        ("amz-sdk-invocation-id".to_string(), Uuid::new_v4().to_string()),
        ("amz-sdk-request".to_string(), "attempt=1; max=1".to_string()),
        ("authorization".to_string(), auth.value.clone()),
        ("connection".to_string(), "close".to_string()),
    ])
}

fn build_kiro_usage_url(auth: &crate::provider_transport::kiro::KiroRequestAuth) -> String {
    let host = format!(
        "q.{}.amazonaws.com",
        auth.auth_config.effective_api_region()
    );
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("origin", "AI_EDITOR");
    serializer.append_pair("resourceType", "AGENTIC_REQUEST");
    serializer.append_pair("isEmailRequired", "true");
    if let Some(profile_arn) = auth.auth_config.profile_arn_for_payload() {
        serializer.append_pair("profileArn", profile_arn);
    }
    format!(
        "https://{host}{KIRO_USAGE_LIMITS_PATH}?{}",
        serializer.finish()
    )
}

async fn execute_kiro_quota_plan(
    state: &AppState,
    transport: &crate::provider_transport::GatewayProviderTransportSnapshot,
    auth: &crate::provider_transport::kiro::KiroRequestAuth,
) -> Result<ProviderQuotaExecutionOutcome, GatewayError> {
    let plan = ExecutionPlan {
        request_id: format!("kiro-quota:{}", transport.key.id),
        candidate_id: None,
        provider_name: Some("kiro".to_string()),
        provider_id: transport.provider.id.clone(),
        endpoint_id: transport.endpoint.id.clone(),
        key_id: transport.key.id.clone(),
        method: "GET".to_string(),
        url: build_kiro_usage_url(auth),
        headers: build_kiro_usage_headers(auth),
        content_type: None,
        content_encoding: None,
        body: RequestBody {
            json_body: None,
            body_bytes_b64: None,
            body_ref: None,
        },
        stream: false,
        client_api_format: "claude:cli".to_string(),
        provider_api_format: "kiro:usage".to_string(),
        model_name: Some("kiro-usage-limits".to_string()),
        proxy: crate::provider_transport::resolve_transport_proxy_snapshot_with_tunnel_affinity(
            state, transport,
        )
        .await,
        tls_profile: crate::provider_transport::resolve_transport_tls_profile(transport),
        timeouts: crate::provider_transport::resolve_transport_execution_timeouts(transport).or(
            Some(ExecutionTimeouts {
                connect_ms: Some(30_000),
                read_ms: Some(30_000),
                write_ms: Some(30_000),
                pool_ms: Some(30_000),
                total_ms: Some(30_000),
                ..ExecutionTimeouts::default()
            }),
        ),
    };

    execute_provider_quota_plan(state, transport, plan, "kiro").await
}

pub(crate) async fn refresh_kiro_provider_quota_locally(
    state: &AppState,
    provider: &StoredProviderCatalogProvider,
    endpoint: &StoredProviderCatalogEndpoint,
    keys: Vec<StoredProviderCatalogKey>,
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

        let Some(auth) = (match state.resolve_local_oauth_request_auth(&transport).await? {
            Some(crate::provider_transport::LocalResolvedOAuthRequestAuth::Kiro(auth)) => {
                Some(auth)
            }
            _ => None,
        }) else {
            failed_count += 1;
            results.push(json!({
                "key_id": key.id,
                "key_name": key.name,
                "status": "error",
                "message": "缺少 Kiro 认证配置 (auth_config)",
            }));
            continue;
        };

        let result = match execute_kiro_quota_plan(state, &transport, &auth).await? {
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
                    let auth_config_json = auth.auth_config.to_json_value().to_string();
                    if let Some(auth_config_json) =
                        encrypt_catalog_secret_with_fallbacks(state, auth_config_json.as_str())
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
