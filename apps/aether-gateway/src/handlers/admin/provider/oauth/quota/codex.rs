use super::shared::{
    coerce_json_bool, coerce_json_f64, coerce_json_string, coerce_json_u64,
    execute_provider_quota_plan, extract_execution_error_message, normalize_string_id_list,
    persist_provider_quota_refresh_state, provider_auto_remove_banned_keys,
    quota_refresh_success_invalid_state, should_auto_remove_structured_reason,
    ProviderQuotaExecutionOutcome,
};
use crate::handlers::admin::provider::shared::payloads::{
    CODEX_WHAM_USAGE_URL, OAUTH_ACCOUNT_BLOCK_PREFIX, OAUTH_EXPIRED_PREFIX,
    OAUTH_REQUEST_FAILED_PREFIX,
};
use crate::{AppState, GatewayError};
use aether_contracts::{ExecutionPlan, ExecutionTimeouts, RequestBody};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

fn normalize_codex_plan_type(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
}

fn build_codex_quota_exhausted_fallback_metadata(
    plan_type: Option<&str>,
    updated_at_unix_secs: u64,
) -> serde_json::Value {
    let mut object = serde_json::Map::new();
    if let Some(plan_type) = normalize_codex_plan_type(plan_type) {
        object.insert(
            "plan_type".to_string(),
            serde_json::Value::String(plan_type),
        );
    }
    object.insert("updated_at".to_string(), json!(updated_at_unix_secs));
    object.insert("primary_used_percent".to_string(), json!(100.0));
    if normalize_codex_plan_type(plan_type) != Some("free".to_string()) {
        object.insert("secondary_used_percent".to_string(), json!(100.0));
    }
    serde_json::Value::Object(object)
}

fn codex_write_window(
    target: &mut serde_json::Map<String, serde_json::Value>,
    source: &serde_json::Map<String, serde_json::Value>,
    target_prefix: &str,
) {
    if let Some(value) = source.get("used_percent").and_then(coerce_json_f64) {
        target.insert(format!("{target_prefix}_used_percent"), json!(value));
    }
    if let Some(value) = source.get("reset_after_seconds").and_then(coerce_json_u64) {
        target.insert(format!("{target_prefix}_reset_after_seconds"), json!(value));
    }
    if let Some(value) = source.get("reset_at").and_then(coerce_json_u64) {
        target.insert(format!("{target_prefix}_reset_at"), json!(value));
    }
    if let Some(value) = source.get("window_minutes").and_then(coerce_json_u64) {
        target.insert(format!("{target_prefix}_window_minutes"), json!(value));
    }
}

fn parse_codex_wham_usage_response(
    value: &serde_json::Value,
    updated_at_unix_secs: u64,
) -> Option<serde_json::Value> {
    let root = value.as_object()?;
    if root.is_empty() {
        return None;
    }

    let mut result = serde_json::Map::new();
    let plan_type =
        normalize_codex_plan_type(root.get("plan_type").and_then(serde_json::Value::as_str));
    if let Some(plan_type) = plan_type.as_ref() {
        result.insert("plan_type".to_string(), json!(plan_type));
    }

    let rate_limit = root
        .get("rate_limit")
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    let primary_window = rate_limit
        .get("primary_window")
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    let secondary_window = rate_limit
        .get("secondary_window")
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();

    let use_paid_windows = !secondary_window.is_empty() && plan_type.as_deref() != Some("free");
    if use_paid_windows {
        codex_write_window(&mut result, &secondary_window, "primary");
        codex_write_window(&mut result, &primary_window, "secondary");
    } else {
        codex_write_window(&mut result, &primary_window, "primary");
    }

    if let Some(credits) = root.get("credits").and_then(serde_json::Value::as_object) {
        if let Some(value) = credits.get("has_credits").and_then(coerce_json_bool) {
            result.insert("has_credits".to_string(), json!(value));
        }
        if let Some(value) = credits.get("balance").and_then(coerce_json_f64) {
            result.insert("credits_balance".to_string(), json!(value));
        }
        if let Some(value) = credits.get("unlimited").and_then(coerce_json_bool) {
            result.insert("credits_unlimited".to_string(), json!(value));
        }
    }

    if result.is_empty() {
        return None;
    }
    result.insert("updated_at".to_string(), json!(updated_at_unix_secs));
    Some(serde_json::Value::Object(result))
}

fn parse_codex_usage_headers(
    headers: &BTreeMap<String, String>,
    updated_at_unix_secs: u64,
) -> Option<serde_json::Value> {
    let mut result = serde_json::Map::new();
    let normalized = headers
        .iter()
        .map(|(key, value)| (key.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect::<BTreeMap<_, _>>();
    if !normalized.keys().any(|key| key.starts_with("x-codex-")) {
        return None;
    }

    let plan_type =
        normalize_codex_plan_type(normalized.get("x-codex-plan-type").map(String::as_str));
    if let Some(plan_type) = plan_type.as_ref() {
        result.insert("plan_type".to_string(), json!(plan_type));
    }

    let read_window = |prefix: &str| -> serde_json::Map<String, serde_json::Value> {
        let mut object = serde_json::Map::new();
        let used_key = format!("x-codex-{prefix}-used-percent");
        let reset_after_key = format!("x-codex-{prefix}-reset-after-seconds");
        let reset_at_key = format!("x-codex-{prefix}-reset-at");
        let window_minutes_key = format!("x-codex-{prefix}-window-minutes");
        if let Some(value) = normalized
            .get(&used_key)
            .and_then(|value| value.parse::<f64>().ok())
        {
            object.insert("used_percent".to_string(), json!(value));
        }
        if let Some(value) = normalized
            .get(&reset_after_key)
            .and_then(|value| value.parse::<u64>().ok())
        {
            object.insert("reset_after_seconds".to_string(), json!(value));
        }
        if let Some(value) = normalized
            .get(&reset_at_key)
            .and_then(|value| value.parse::<u64>().ok())
        {
            object.insert("reset_at".to_string(), json!(value));
        }
        if let Some(value) = normalized
            .get(&window_minutes_key)
            .and_then(|value| value.parse::<u64>().ok())
        {
            object.insert("window_minutes".to_string(), json!(value));
        }
        object
    };

    let primary_window = read_window("primary");
    let secondary_window = read_window("secondary");
    let use_paid_windows = !secondary_window.is_empty() && plan_type.as_deref() != Some("free");
    if use_paid_windows {
        codex_write_window(&mut result, &secondary_window, "primary");
        codex_write_window(&mut result, &primary_window, "secondary");
    } else {
        codex_write_window(&mut result, &primary_window, "primary");
    }

    if let Some(value) = normalized
        .get("x-codex-primary-over-secondary-limit-percent")
        .and_then(|value| value.parse::<f64>().ok())
    {
        result.insert(
            "primary_over_secondary_limit_percent".to_string(),
            json!(value),
        );
    }
    if let Some(value) = normalized
        .get("x-codex-credits-has-credits")
        .and_then(|value| match value.to_ascii_lowercase().as_str() {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => None,
        })
    {
        result.insert("has_credits".to_string(), json!(value));
    }
    if let Some(value) = normalized
        .get("x-codex-credits-balance")
        .and_then(|value| value.parse::<f64>().ok())
    {
        result.insert("credits_balance".to_string(), json!(value));
    }
    if let Some(value) = normalized
        .get("x-codex-credits-unlimited")
        .and_then(|value| match value.to_ascii_lowercase().as_str() {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => None,
        })
    {
        result.insert("credits_unlimited".to_string(), json!(value));
    }

    if result.is_empty() {
        return None;
    }
    result.insert("updated_at".to_string(), json!(updated_at_unix_secs));
    Some(serde_json::Value::Object(result))
}

fn codex_current_invalid_reason(key: &StoredProviderCatalogKey) -> String {
    key.oauth_invalid_reason
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .to_string()
}

fn codex_merge_invalid_reason(current: &str, candidate_reason: &str) -> String {
    if current.is_empty() {
        return candidate_reason.to_string();
    }
    if current.starts_with(OAUTH_ACCOUNT_BLOCK_PREFIX) {
        return current.to_string();
    }
    if current.starts_with(OAUTH_EXPIRED_PREFIX)
        && candidate_reason.starts_with(OAUTH_REQUEST_FAILED_PREFIX)
    {
        return current.to_string();
    }
    candidate_reason.to_string()
}

fn codex_build_invalid_state(
    key: &StoredProviderCatalogKey,
    candidate_reason: String,
    now_unix_secs: u64,
) -> (Option<u64>, Option<String>) {
    let current_reason = codex_current_invalid_reason(key);
    let merged_reason = codex_merge_invalid_reason(&current_reason, &candidate_reason);
    if merged_reason == current_reason {
        return (key.oauth_invalid_at_unix_secs, Some(merged_reason));
    }
    (Some(now_unix_secs), Some(merged_reason))
}

fn codex_looks_like_token_invalidated(message: Option<&str>) -> bool {
    let lowered = message.unwrap_or_default().trim().to_ascii_lowercase();
    lowered.contains("token invalid")
        || lowered.contains("token invalidated")
        || lowered.contains("session has expired")
        || lowered.contains("session expired")
}

fn codex_looks_like_account_deactivated(message: Option<&str>) -> bool {
    let lowered = message.unwrap_or_default().trim().to_ascii_lowercase();
    lowered.contains("account has been deactivated") || lowered.contains("account deactivated")
}

fn codex_looks_like_workspace_deactivated(message: Option<&str>) -> bool {
    let lowered = message.unwrap_or_default().trim().to_ascii_lowercase();
    lowered.contains("deactivated_workspace")
        || (lowered.contains("workspace") && lowered.contains("deactivated"))
}

fn codex_structured_invalid_reason(status_code: u16, upstream_message: Option<&str>) -> String {
    let message = upstream_message.unwrap_or_default().trim();
    if status_code == 402 && codex_looks_like_workspace_deactivated(Some(message)) {
        return format!("{OAUTH_ACCOUNT_BLOCK_PREFIX}工作区已停用 (deactivated_workspace)");
    }
    if codex_looks_like_account_deactivated(Some(message)) {
        let detail = if message.is_empty() {
            "OpenAI 账号已停用"
        } else {
            message
        };
        return format!("{OAUTH_ACCOUNT_BLOCK_PREFIX}{detail}");
    }
    if codex_looks_like_token_invalidated(Some(message)) {
        let detail = if message.is_empty() {
            "Codex Token 无效或已过期"
        } else {
            message
        };
        return format!("{OAUTH_EXPIRED_PREFIX}{detail}");
    }
    if status_code == 401 {
        let detail = if message.is_empty() {
            "Codex Token 无效或已过期 (401)"
        } else {
            message
        };
        return format!("{OAUTH_EXPIRED_PREFIX}{detail}");
    }
    if status_code == 403 {
        let detail = if message.is_empty() {
            "Codex 账户访问受限 (403)"
        } else {
            message
        };
        return format!("{OAUTH_ACCOUNT_BLOCK_PREFIX}{detail}");
    }
    message.to_string()
}

fn codex_soft_request_failure_reason(status_code: u16, upstream_message: Option<&str>) -> String {
    let detail = upstream_message
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("Codex 请求失败 ({status_code})"));
    format!("{OAUTH_REQUEST_FAILED_PREFIX}{detail}")
}

fn build_codex_refresh_headers(
    transport: &crate::provider_transport::GatewayProviderTransportSnapshot,
    resolved_oauth_auth: Option<(String, String)>,
) -> Result<BTreeMap<String, String>, String> {
    let mut headers = BTreeMap::new();
    headers.insert("accept".to_string(), "application/json".to_string());

    if let Some((name, value)) = resolved_oauth_auth {
        headers.insert(name.to_ascii_lowercase(), value);
    } else {
        let decrypted_key = transport.key.decrypted_api_key.trim();
        if decrypted_key.is_empty() || decrypted_key == "__placeholder__" {
            return Err("缺少 OAuth 认证信息，请先授权/刷新 Token".to_string());
        }
        headers.insert(
            "authorization".to_string(),
            format!("Bearer {decrypted_key}"),
        );
    }

    let auth_config = transport
        .key
        .decrypted_auth_config
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok());
    let oauth_plan_type = normalize_codex_plan_type(
        auth_config
            .as_ref()
            .and_then(|value| value.get("plan_type"))
            .and_then(serde_json::Value::as_str),
    );
    let oauth_account_id = auth_config
        .as_ref()
        .and_then(|value| value.get("account_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if oauth_account_id.is_some() && oauth_plan_type.as_deref() != Some("free") {
        headers.insert(
            "chatgpt-account-id".to_string(),
            oauth_account_id.unwrap_or_default().to_string(),
        );
    }

    Ok(headers)
}

async fn execute_codex_quota_plan(
    state: &AppState,
    transport: &crate::provider_transport::GatewayProviderTransportSnapshot,
    headers: BTreeMap<String, String>,
) -> Result<ProviderQuotaExecutionOutcome, GatewayError> {
    let plan = ExecutionPlan {
        request_id: format!("codex-quota:{}", transport.key.id),
        candidate_id: None,
        provider_name: Some("codex".to_string()),
        provider_id: transport.provider.id.clone(),
        endpoint_id: transport.endpoint.id.clone(),
        key_id: transport.key.id.clone(),
        method: "GET".to_string(),
        url: CODEX_WHAM_USAGE_URL.to_string(),
        headers,
        content_type: None,
        content_encoding: None,
        body: RequestBody {
            json_body: None,
            body_bytes_b64: None,
            body_ref: None,
        },
        stream: false,
        client_api_format: "openai:cli".to_string(),
        provider_api_format: "openai:cli".to_string(),
        model_name: Some("codex-wham-usage".to_string()),
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
    execute_provider_quota_plan(state, transport, plan, "codex").await
}

pub(crate) async fn refresh_codex_provider_quota_locally(
    state: &AppState,
    provider: &StoredProviderCatalogProvider,
    endpoint: &StoredProviderCatalogEndpoint,
    keys: Vec<StoredProviderCatalogKey>,
) -> Result<Option<serde_json::Value>, GatewayError> {
    let auto_remove_abnormal_keys = provider_auto_remove_banned_keys(provider.config.as_ref());
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

        let resolved_oauth_auth = if key.auth_type.trim().eq_ignore_ascii_case("oauth") {
            match state.resolve_local_oauth_request_auth(&transport).await? {
                Some(crate::provider_transport::LocalResolvedOAuthRequestAuth::Header {
                    name,
                    value,
                }) => Some((name, value)),
                _ => None,
            }
        } else {
            None
        };

        let headers = match build_codex_refresh_headers(&transport, resolved_oauth_auth) {
            Ok(headers) => headers,
            Err(message) => {
                failed_count += 1;
                results.push(json!({
                    "key_id": key.id,
                    "key_name": key.name,
                    "status": "error",
                    "message": message,
                }));
                continue;
            }
        };

        let result = match execute_codex_quota_plan(state, &transport, headers).await? {
            ProviderQuotaExecutionOutcome::Response(result) => result,
            ProviderQuotaExecutionOutcome::Failure(detail) => {
                failed_count += 1;
                results.push(json!({
                    "key_id": key.id,
                    "key_name": key.name,
                    "status": "error",
                    "message": format!("wham/usage 请求执行失败: {detail}"),
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

        let mut metadata_update = parse_codex_usage_headers(&result.headers, now_unix_secs)
            .map(|metadata| json!({ "codex": metadata }));
        let (mut oauth_invalid_at_unix_secs, mut oauth_invalid_reason) = (None, None);
        let mut status = "error".to_string();
        let mut message = None::<String>;
        let mut status_code = Some(result.status_code);

        if result.status_code == 200 {
            if let Some(body_json) = result
                .body
                .as_ref()
                .and_then(|body| body.json_body.as_ref())
            {
                if let Some(parsed) = parse_codex_wham_usage_response(body_json, now_unix_secs) {
                    metadata_update = Some(json!({ "codex": parsed }));
                    (oauth_invalid_at_unix_secs, oauth_invalid_reason) =
                        quota_refresh_success_invalid_state(&key);
                    status = "success".to_string();
                } else {
                    status = "no_metadata".to_string();
                    message = Some("响应中未包含限额信息".to_string());
                }
            } else {
                message = Some("无法解析 wham/usage API 响应".to_string());
            }
        } else {
            let err_msg = extract_execution_error_message(&result);
            message = Some(match err_msg.as_deref() {
                Some(detail) if !detail.is_empty() => {
                    format!(
                        "wham/usage API 返回状态码 {}: {}",
                        result.status_code, detail
                    )
                }
                _ => format!("wham/usage API 返回状态码 {}", result.status_code),
            });

            match result.status_code {
                401 => {
                    let (at, reason) = codex_build_invalid_state(
                        &key,
                        codex_structured_invalid_reason(401, err_msg.as_deref()),
                        now_unix_secs,
                    );
                    oauth_invalid_at_unix_secs = at;
                    oauth_invalid_reason = reason;
                    status = "auth_invalid".to_string();
                }
                402 => {
                    if codex_looks_like_workspace_deactivated(err_msg.as_deref()) {
                        let mut codex_meta = metadata_update
                            .as_ref()
                            .and_then(|value| value.get("codex"))
                            .and_then(serde_json::Value::as_object)
                            .cloned()
                            .unwrap_or_default();
                        codex_meta.insert("updated_at".to_string(), json!(now_unix_secs));
                        codex_meta.insert("account_disabled".to_string(), json!(true));
                        codex_meta.insert("reason".to_string(), json!("deactivated_workspace"));
                        codex_meta.insert(
                            "message".to_string(),
                            json!(err_msg
                                .clone()
                                .unwrap_or_else(|| "deactivated_workspace".to_string())),
                        );
                        let plan_type = transport
                            .key
                            .decrypted_auth_config
                            .as_deref()
                            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
                            .and_then(|value| {
                                value
                                    .get("plan_type")
                                    .and_then(serde_json::Value::as_str)
                                    .map(ToOwned::to_owned)
                            });
                        if let Some(plan_type) = plan_type {
                            codex_meta
                                .entry("plan_type".to_string())
                                .or_insert_with(|| json!(plan_type.to_ascii_lowercase()));
                        }
                        metadata_update = Some(json!({ "codex": codex_meta }));
                        let (at, reason) = codex_build_invalid_state(
                            &key,
                            codex_structured_invalid_reason(402, err_msg.as_deref()),
                            now_unix_secs,
                        );
                        oauth_invalid_at_unix_secs = at;
                        oauth_invalid_reason = reason;
                        status = "workspace_deactivated".to_string();
                    } else {
                        let plan_type = transport
                            .key
                            .decrypted_auth_config
                            .as_deref()
                            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
                            .and_then(|value| {
                                value
                                    .get("plan_type")
                                    .and_then(serde_json::Value::as_str)
                                    .map(ToOwned::to_owned)
                            });
                        metadata_update = Some(json!({
                            "codex": build_codex_quota_exhausted_fallback_metadata(
                                plan_type.as_deref(),
                                now_unix_secs,
                            )
                        }));
                        (oauth_invalid_at_unix_secs, oauth_invalid_reason) =
                            quota_refresh_success_invalid_state(&key);
                        status = "quota_exhausted".to_string();
                    }
                }
                403 => {
                    let candidate_reason = if codex_looks_like_token_invalidated(err_msg.as_deref())
                    {
                        codex_structured_invalid_reason(403, err_msg.as_deref())
                    } else {
                        codex_soft_request_failure_reason(403, err_msg.as_deref())
                    };
                    let (at, reason) =
                        codex_build_invalid_state(&key, candidate_reason, now_unix_secs);
                    oauth_invalid_at_unix_secs = at;
                    oauth_invalid_reason = reason;
                    status = "forbidden".to_string();
                }
                _ => {}
            }
        }

        let auto_removed = auto_remove_abnormal_keys
            && should_auto_remove_structured_reason(oauth_invalid_reason.as_deref());
        if auto_removed {
            if state.delete_provider_catalog_key(&key.id).await? {
                auto_removed_count += 1;
            }
        } else if !persist_provider_quota_refresh_state(
            state,
            &key.id,
            metadata_update.as_ref(),
            oauth_invalid_at_unix_secs,
            oauth_invalid_reason.clone(),
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
        if let Some(status_code) = status_code.take() {
            if status_code != 200 {
                payload.insert("status_code".to_string(), json!(status_code));
            }
        }
        if let Some(metadata_update) = metadata_update
            .as_ref()
            .and_then(|value| value.get("codex"))
            .cloned()
        {
            payload.insert("metadata".to_string(), metadata_update);
        }
        if auto_removed {
            payload.insert("auto_removed".to_string(), json!(true));
        }
        results.push(serde_json::Value::Object(payload));
    }

    Ok(Some(json!({
        "success": success_count,
        "failed": failed_count,
        "total": results.len(),
        "results": results,
        "auto_removed": auto_removed_count,
    })))
}
