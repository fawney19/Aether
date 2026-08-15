use super::super::super::config::persist_admin_provider_ops_runtime_credentials;
use super::super::super::verify::{
    admin_provider_ops_execute_json_request, admin_provider_ops_sub2api_exchange_token,
    admin_provider_ops_sub2api_request_url, AdminProviderOpsExecuteJsonError,
};
use super::super::responses::{
    admin_provider_ops_action_error, admin_provider_ops_action_response,
};
use super::super::support::admin_provider_ops_json_object_map;
use crate::handlers::admin::request::AdminAppState;
use crate::handlers::shared::system_config_bool;
use aether_admin::provider::ops::{
    parse_sub2api_balance_payload, parse_sub2api_remote_quota, parse_sub2api_remote_quota_groups,
    validate_sub2api_same_origin_endpoint, Sub2ApiRemoteQuotaConfig, Sub2ApiRemoteQuotaSnapshot,
};
use aether_admin::system::ENABLE_PROVIDER_REMOTE_QUOTA_SYNC_CONFIG_KEY;
use aether_contracts::ProxySnapshot;
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider;
use aether_data_contracts::repository::quota::{
    ApplyRemoteProviderQuotaOutcome, ApplyRemoteProviderQuotaPatch, ProviderQuotaUsageObservation,
};
use aether_runtime_state::RuntimeLockLease;
use serde_json::{json, Value};
use std::time::Duration;
use tracing::warn;
use uuid::Uuid;

const REMOTE_QUOTA_SYNC_LOCK_TTL: Duration = Duration::from_secs(120);

pub(super) async fn admin_provider_ops_sub2api_balance_payload(
    state: &AdminAppState<'_>,
    provider_id: &str,
    provider: &StoredProviderCatalogProvider,
    base_url: &str,
    action_config: &serde_json::Map<String, serde_json::Value>,
    credentials: &serde_json::Map<String, serde_json::Value>,
    proxy_snapshot: Option<&ProxySnapshot>,
    remote_quota_config: Option<&Sub2ApiRemoteQuotaConfig>,
) -> serde_json::Value {
    if remote_quota_config.is_none() {
        return admin_provider_ops_sub2api_balance_payload_inner(
            state,
            provider_id,
            provider,
            base_url,
            action_config,
            credentials,
            proxy_snapshot,
            remote_quota_config,
            None,
        )
        .await;
    }

    let lock_key = format!("provider_ops:remote_quota_sync:{provider_id}");
    let lock_owner = format!("aether-gateway-remote-quota-{}", Uuid::new_v4());
    let lock = match state
        .runtime_state()
        .lock_try_acquire(&lock_key, &lock_owner, REMOTE_QUOTA_SYNC_LOCK_TTL)
        .await
    {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            return admin_provider_ops_action_error(
                "unknown_error",
                "query_balance",
                "该 Provider 的远程额度同步正在进行，请稍后重试".to_string(),
                None,
            );
        }
        Err(error) => {
            warn!(
                provider_id = %provider_id,
                error = ?error,
                "failed to acquire remote quota sync lock"
            );
            return admin_provider_ops_action_error(
                "unknown_error",
                "query_balance",
                "远程额度同步锁不可用，本地额度保持不变".to_string(),
                None,
            );
        }
    };

    let payload = admin_provider_ops_sub2api_balance_payload_inner(
        state,
        provider_id,
        provider,
        base_url,
        action_config,
        credentials,
        proxy_snapshot,
        remote_quota_config,
        Some(&lock),
    )
    .await;

    if let Err(error) = state.runtime_state().lock_release(&lock).await {
        warn!(
            provider_id = %provider_id,
            error = ?error,
            "failed to release remote quota sync lock"
        );
    }
    payload
}

async fn admin_provider_ops_sub2api_balance_payload_inner(
    state: &AdminAppState<'_>,
    provider_id: &str,
    provider: &StoredProviderCatalogProvider,
    base_url: &str,
    action_config: &serde_json::Map<String, serde_json::Value>,
    credentials: &serde_json::Map<String, serde_json::Value>,
    proxy_snapshot: Option<&ProxySnapshot>,
    remote_quota_config: Option<&Sub2ApiRemoteQuotaConfig>,
    remote_quota_lock: Option<&RuntimeLockLease>,
) -> serde_json::Value {
    let start = std::time::Instant::now();
    let (access_token, updated_credentials, _frontend_updated_credentials) =
        match admin_provider_ops_sub2api_exchange_token(
            state,
            base_url,
            credentials,
            proxy_snapshot,
        )
        .await
        {
            Ok(value) => value,
            Err(message) => {
                return admin_provider_ops_action_error(
                    "auth_failed",
                    "query_balance",
                    message,
                    None,
                );
            }
        };

    if !updated_credentials.is_empty() {
        if let Err(err) =
            persist_admin_provider_ops_runtime_credentials(state, provider, &updated_credentials)
                .await
        {
            warn!(
                provider_id = %provider_id,
                error = ?err,
                "failed to persist sub2api runtime credentials"
            );
        }
    }

    let me_endpoint = action_config
        .get("endpoint")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("/api/v1/auth/me?timezone=Asia/Shanghai");
    let me_url = admin_provider_ops_sub2api_request_url(base_url, me_endpoint);
    let subscription_endpoint = admin_provider_ops_json_object_map(json!({
        "endpoint": action_config
            .get("subscription_endpoint")
            .cloned()
            .unwrap_or_else(|| json!("/api/v1/subscriptions/summary")),
    }))
    .get("endpoint")
    .and_then(serde_json::Value::as_str)
    .map(str::trim)
    .filter(|value| !value.is_empty())
    .unwrap_or("/api/v1/subscriptions/summary")
    .to_string();
    if remote_quota_config.is_some() {
        for endpoint in [me_endpoint, subscription_endpoint.as_str()] {
            if let Err(message) = validate_sub2api_same_origin_endpoint(endpoint) {
                return admin_provider_ops_action_error(
                    "parse_error",
                    "query_balance",
                    message,
                    None,
                );
            }
        }
    }
    let subscription_url =
        admin_provider_ops_sub2api_request_url(base_url, subscription_endpoint.as_str());
    let progress_url = remote_quota_config.map(|config| {
        admin_provider_ops_sub2api_request_url(base_url, config.progress_endpoint.as_str())
    });
    // Observe local usage immediately before the authoritative remote fetch.
    // The repository later replaces older local estimates with the remote
    // absolute value while retaining only usage added during this fetch.
    let local_usage_observation = if remote_quota_config.is_some() {
        Some(
            match state
                .app()
                .read_provider_quota_snapshot_strong(provider_id)
                .await
            {
                Ok(Some(snapshot)) => Ok(ProviderQuotaUsageObservation::from(&snapshot)),
                Ok(None) => Err("读取本地 Provider 配额失败: Provider 不存在".to_string()),
                Err(error) => Err(format!(
                    "读取本地 Provider 配额失败: {}",
                    error.into_message()
                )),
            },
        )
    } else {
        None
    };

    let auth_value = match reqwest::header::HeaderValue::from_str(&format!("Bearer {access_token}"))
    {
        Ok(value) => value,
        Err(_) => {
            return admin_provider_ops_action_error(
                "parse_error",
                "query_balance",
                "访问令牌格式无效",
                None,
            );
        }
    };
    let auth_headers =
        reqwest::header::HeaderMap::from_iter([(reqwest::header::AUTHORIZATION, auth_value)]);
    let me_request_id = format!("provider-ops-action:sub2api:me:{provider_id}");
    let subscription_request_id =
        format!("provider-ops-action:sub2api:subscriptions:{provider_id}");
    let progress_request_id = format!("provider-ops-action:sub2api:progress:{provider_id}");
    let progress_request = async {
        let Some(progress_url) = progress_url.as_deref() else {
            return Ok(None);
        };
        admin_provider_ops_execute_json_request(
            state,
            &progress_request_id,
            reqwest::Method::GET,
            progress_url,
            &auth_headers,
            None,
            proxy_snapshot,
        )
        .await
        .map(Some)
    };
    let (me_result, subscription_result, progress_result) = tokio::join!(
        admin_provider_ops_execute_json_request(
            state,
            &me_request_id,
            reqwest::Method::GET,
            &me_url,
            &auth_headers,
            None,
            proxy_snapshot,
        ),
        admin_provider_ops_execute_json_request(
            state,
            &subscription_request_id,
            reqwest::Method::GET,
            &subscription_url,
            &auth_headers,
            None,
            proxy_snapshot,
        ),
        progress_request
    );
    if let Some(lock) = remote_quota_lock {
        match state
            .runtime_state()
            .lock_renew(lock, REMOTE_QUOTA_SYNC_LOCK_TTL)
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                return admin_provider_ops_action_error(
                    "unknown_error",
                    "query_balance",
                    "远程额度同步锁已失效，本地额度保持不变".to_string(),
                    None,
                );
            }
            Err(error) => {
                warn!(
                    provider_id = %provider_id,
                    error = ?error,
                    "failed to renew remote quota sync lock"
                );
                return admin_provider_ops_action_error(
                    "unknown_error",
                    "query_balance",
                    "远程额度同步锁续期失败，本地额度保持不变".to_string(),
                    None,
                );
            }
        }
    }
    let me_result = me_result.map_err(|err| match err {
        AdminProviderOpsExecuteJsonError::InvalidJson(message)
        | AdminProviderOpsExecuteJsonError::Transport(message) => message,
    });
    let subscription_result = subscription_result.map_err(json_execution_error_message);
    let progress_result = progress_result.map_err(json_execution_error_message);
    let response_time_ms = Some(start.elapsed().as_millis() as u64);

    let (me_status, me_json) = match me_result {
        Ok(result) => result,
        Err(err) => {
            return admin_provider_ops_action_error(
                "network_error",
                "query_balance",
                network_error_message(&err),
                response_time_ms,
            );
        }
    };
    if matches!(
        me_status,
        http::StatusCode::UNAUTHORIZED | http::StatusCode::FORBIDDEN
    ) {
        return admin_provider_ops_action_error(
            "auth_failed",
            "query_balance",
            "认证失败，请检查凭据配置",
            response_time_ms,
        );
    }
    if me_status != http::StatusCode::OK {
        return admin_provider_ops_action_error(
            "unknown_error",
            "query_balance",
            format!(
                "HTTP {}: {}",
                me_status.as_u16(),
                me_status.canonical_reason().unwrap_or("Unknown")
            ),
            response_time_ms,
        );
    }

    let (subscription_json, subscription_failure) = match subscription_result {
        Ok((status, payload)) if status == http::StatusCode::OK => (Some(payload), None),
        Ok((status, _)) => (
            None,
            Some(format!(
                "Sub2API 套餐摘要请求失败: HTTP {}",
                status.as_u16()
            )),
        ),
        Err(message) => (None, Some(network_error_message(&message))),
    };
    // Progress is enrichment only: summary still authoritatively classifies an
    // unlimited or missing Group when this endpoint is unavailable. A limited
    // subscription will fail closed later because its window cannot be proven.
    let (progress_json, progress_warning) = match progress_result {
        Ok(Some((status, payload))) if status == http::StatusCode::OK => (Some(payload), None),
        Ok(Some((status, _))) => (
            None,
            Some(format!(
                "Sub2API 套餐进度请求失败: HTTP {}",
                status.as_u16()
            )),
        ),
        Err(message) => (None, Some(network_error_message(&message))),
        Ok(None) => (None, None),
    };
    let mut data =
        match parse_sub2api_balance_payload(action_config, &me_json, subscription_json.as_ref()) {
            Ok(data) => data,
            Err(message) => {
                return admin_provider_ops_action_error(
                    if message == "响应格式无效" {
                        "parse_error"
                    } else {
                        "unknown_error"
                    },
                    "query_balance",
                    message,
                    response_time_ms,
                );
            }
        };

    if let Some(remote_quota_config) = remote_quota_config {
        let remote_subscription = subscription_json
            .as_ref()
            .and_then(|payload| parse_sub2api_remote_quota_groups(payload).ok())
            .and_then(|groups| {
                groups
                    .into_iter()
                    .find(|group| group.group_id == remote_quota_config.group_id)
            });
        let mut sync_status = if let Some(message) = subscription_failure {
            remote_quota_failed_keep_local(message, progress_warning)
        } else if let Some(subscription_json) = subscription_json.as_ref() {
            match parse_sub2api_remote_quota(
                subscription_json,
                progress_json.as_ref(),
                remote_quota_config.group_id.as_str(),
            ) {
                Ok(snapshot) => match apply_remote_quota(
                    state,
                    provider_id,
                    snapshot,
                    local_usage_observation
                        .as_ref()
                        .expect("remote quota fetch has a local usage observation result"),
                )
                .await
                {
                    Ok(mut status) => {
                        if let Some(warning) = progress_warning {
                            status["warning"] = Value::String(warning);
                        }
                        status
                    }
                    Err(message) => remote_quota_failed_keep_local(message, progress_warning),
                },
                Err(message) => remote_quota_failed_keep_local(message, progress_warning),
            }
        } else {
            remote_quota_failed_keep_local("Sub2API 套餐摘要响应缺失".to_string(), progress_warning)
        };
        if let Some(remote_subscription) = remote_subscription {
            let mut remote_subscription = json!(remote_subscription);
            let applied_window = sync_status
                .pointer("/remote/window")
                .and_then(Value::as_str)
                .map(str::to_string);
            let applied_used_usd = sync_status.pointer("/remote/remote_used_usd").cloned();
            let applied_limit_usd = sync_status.pointer("/remote/limit_usd").cloned();
            if let (Some(window), Some(subscription)) =
                (applied_window, remote_subscription.as_object_mut())
            {
                subscription.insert(
                    "local_sync_window".to_string(),
                    Value::String(window.clone()),
                );
                if let Some(used_usd) = applied_used_usd {
                    subscription.insert(format!("{window}_used_usd"), used_usd);
                }
                if let Some(limit_usd) = applied_limit_usd {
                    subscription.insert(format!("{window}_limit_usd"), limit_usd);
                }
            }
            sync_status["subscription"] = remote_subscription;
        }
        attach_remote_quota_sync_status(&mut data, sync_status);
    }

    admin_provider_ops_action_response(
        "success",
        "query_balance",
        data,
        None,
        response_time_ms,
        86400,
    )
}

async fn apply_remote_quota(
    state: &AdminAppState<'_>,
    provider_id: &str,
    snapshot: Sub2ApiRemoteQuotaSnapshot,
    local_usage_observation: &Result<ProviderQuotaUsageObservation, String>,
) -> Result<Value, String> {
    let kill_switch = state
        .app()
        .read_system_config_json_value(ENABLE_PROVIDER_REMOTE_QUOTA_SYNC_CONFIG_KEY)
        .await
        .map_err(|error| format!("读取远程额度同步开关失败: {}", error.into_message()))?;
    if !system_config_bool(kill_switch.as_ref(), true) {
        return Ok(json!({
            "status": "skipped_kill_switch",
            "message": "远程额度同步已被全局开关暂停，本地额度保持不变"
        }));
    }
    let observed_at = chrono::Utc::now().timestamp().max(1) as u64;
    let (patch, detail) = match snapshot {
        Sub2ApiRemoteQuotaSnapshot::ActiveLimited {
            group_id,
            group_name,
            subscription_id,
            window,
            limit_usd,
            used_usd,
            window_start_unix_secs,
            resets_at_unix_secs,
            expires_at_unix_secs,
        } => (
            ApplyRemoteProviderQuotaPatch {
                provider_id: provider_id.to_string(),
                billing_type: "monthly_quota".to_string(),
                monthly_quota_usd: Some(limit_usd),
                remote_monthly_used_usd: used_usd,
                remote_window_start_unix_secs: window_start_unix_secs,
                remote_window_end_unix_secs: resets_at_unix_secs,
                // Remote progress owns the exact reset boundary. Keeping the
                // local interval unset prevents the generic reset worker from
                // reopening quota before the next authoritative sync.
                quota_reset_day: None,
                quota_expires_at_unix_secs: expires_at_unix_secs,
                local_usage_observation: Some(local_usage_observation.clone()?),
                preserve_local_used_usd: false,
            },
            json!({
                "group_id": group_id,
                "group_name": group_name,
                "subscription_id": subscription_id,
                "classification": "active_limited",
                "window": window.as_str(),
                "limit_usd": limit_usd,
                "remote_used_usd": used_usd,
                "window_start_unix_secs": window_start_unix_secs,
                "resets_at_unix_secs": resets_at_unix_secs,
                "expires_at_unix_secs": expires_at_unix_secs,
            }),
        ),
        Sub2ApiRemoteQuotaSnapshot::ActiveUnlimited {
            group_id,
            group_name,
            subscription_id,
            expires_at_unix_secs,
        } => {
            // 状态覆盖，用量保留：unlimited 不代表本地已用量归零。
            (
                ApplyRemoteProviderQuotaPatch {
                    provider_id: provider_id.to_string(),
                    billing_type: "pay_as_you_go".to_string(),
                    monthly_quota_usd: None,
                    remote_monthly_used_usd: 0.0,
                    remote_window_start_unix_secs: observed_at,
                    remote_window_end_unix_secs: observed_at.saturating_add(1),
                    quota_reset_day: None,
                    quota_expires_at_unix_secs: expires_at_unix_secs,
                    local_usage_observation: None,
                    preserve_local_used_usd: true,
                },
                json!({
                    "group_id": group_id,
                    "group_name": group_name,
                    "subscription_id": subscription_id,
                    "classification": "active_unlimited",
                    "expires_at_unix_secs": expires_at_unix_secs,
                }),
            )
        }
        Sub2ApiRemoteQuotaSnapshot::Exhausted { group_id } => {
            // 状态覆盖，用量保留：套餐缺失不代表本地已用量归零。
            (
                ApplyRemoteProviderQuotaPatch {
                    provider_id: provider_id.to_string(),
                    billing_type: "monthly_quota".to_string(),
                    monthly_quota_usd: Some(0.0),
                    remote_monthly_used_usd: 0.0,
                    remote_window_start_unix_secs: observed_at,
                    remote_window_end_unix_secs: observed_at.saturating_add(1),
                    quota_reset_day: None,
                    quota_expires_at_unix_secs: None,
                    local_usage_observation: None,
                    preserve_local_used_usd: true,
                },
                json!({
                    "group_id": group_id,
                    "classification": "exhausted",
                }),
            )
        }
    };
    let outcome = state
        .app()
        .apply_remote_provider_quota(&patch)
        .await
        .map_err(|error| format!("写入本地 Provider 配额失败: {}", error.into_message()))?;
    match outcome {
        ApplyRemoteProviderQuotaOutcome::Applied(local) => {
            let mut local_detail = json!({
                "billing_type": local.billing_type,
                "monthly_quota_usd": local.monthly_quota_usd,
                "monthly_used_usd": local.monthly_used_usd,
                "quota_last_reset_at_unix_secs": local.quota_last_reset_at_unix_secs,
                "quota_expires_at_unix_secs": local.quota_expires_at_unix_secs,
            });
            if let Some(remote_confirmed_used_usd) =
                detail.get("remote_used_usd").and_then(Value::as_f64)
            {
                local_detail["remote_confirmed_used_usd"] = json!(remote_confirmed_used_usd);
                local_detail["pending_local_used_usd"] =
                    json!((local.monthly_used_usd - remote_confirmed_used_usd).max(0.0));
            }
            Ok(json!({
                "status": "applied",
                "remote": detail,
                "local": local_detail,
            }))
        }
        ApplyRemoteProviderQuotaOutcome::StaleWindow(local) => Ok(json!({
            "status": "stale_window",
            "message": "远程套餐窗口早于本地已同步窗口，本地额度保持不变",
            "remote": detail,
            "local": {
                "billing_type": local.billing_type,
                "monthly_quota_usd": local.monthly_quota_usd,
                "monthly_used_usd": local.monthly_used_usd,
                "quota_last_reset_at_unix_secs": local.quota_last_reset_at_unix_secs,
                "quota_expires_at_unix_secs": local.quota_expires_at_unix_secs,
            }
        })),
        ApplyRemoteProviderQuotaOutcome::ConcurrentModification(local) => Ok(json!({
            "status": "concurrent_update",
            "message": "同步期间本地额度已变化，本次远程快照未应用",
            "remote": detail,
            "local": {
                "billing_type": local.billing_type,
                "monthly_quota_usd": local.monthly_quota_usd,
                "monthly_used_usd": local.monthly_used_usd,
                "quota_last_reset_at_unix_secs": local.quota_last_reset_at_unix_secs,
                "quota_expires_at_unix_secs": local.quota_expires_at_unix_secs,
            }
        })),
        ApplyRemoteProviderQuotaOutcome::ProviderNotFound => {
            Err("写入本地 Provider 配额失败: Provider 不存在".to_string())
        }
    }
}

fn attach_remote_quota_sync_status(data: &mut Value, status: Value) {
    let Some(data) = data.as_object_mut() else {
        return;
    };
    let extra = data
        .entry("extra")
        .or_insert_with(|| json!({}))
        .as_object_mut();
    if let Some(extra) = extra {
        extra.insert("remote_quota_sync".to_string(), status);
    }
}

fn remote_quota_failed_keep_local(message: String, warning: Option<String>) -> Value {
    let mut status = json!({
        "status": "failed_keep_local",
        "message": message,
    });
    if let Some(warning) = warning {
        status["warning"] = Value::String(warning);
    }
    status
}

fn json_execution_error_message(error: AdminProviderOpsExecuteJsonError) -> String {
    match error {
        AdminProviderOpsExecuteJsonError::InvalidJson(message)
        | AdminProviderOpsExecuteJsonError::Transport(message) => message,
    }
}

fn network_error_message(error: &str) -> String {
    let normalized = error.trim();
    let lower = normalized.to_ascii_lowercase();
    if lower.contains("timeout") || normalized.contains("超时") {
        return "请求超时".to_string();
    }
    if normalized.starts_with("网络错误:") {
        return normalized.to_string();
    }
    format!("网络错误: {normalized}")
}
