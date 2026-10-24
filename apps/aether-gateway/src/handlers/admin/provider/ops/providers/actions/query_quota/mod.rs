//! Custom-provider quota query action.
//!
//! Unlike `query_balance`, this action is driven entirely by the provider's own
//! `config.quota_query` block (custom providers only) instead of the Provider
//! Ops config. The block holds a Rhai script defining the HTTP request (a
//! full URL whose headers may embed `{{apiKey}}` placeholders resolved from
//! the provider's Provider Ops credentials) and an extractor closure that
//! maps the JSON response onto display fields, including optional 5h / week /
//! month periods. The result is cached for an hour.

use super::super::config::admin_provider_ops_credential_snapshot;
use super::super::verify::{
    admin_provider_ops_execute_json_request, AdminProviderOpsExecuteJsonError,
};
use super::responses::{admin_provider_ops_action_error, admin_provider_ops_action_response};
use crate::handlers::admin::request::AdminAppState;
use aether_admin::provider::ops::quota_script::{
    run_quota_extractor, validate_quota_script, QuotaScriptRequest,
};
use aether_data_contracts::repository::provider_catalog::{
    ProviderCatalogKeyStatusSnapshotUpdate, StoredProviderCatalogEndpoint,
    StoredProviderCatalogProvider,
};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::time::Duration;

pub(crate) const ADMIN_PROVIDER_OPS_QUOTA_CACHE_PREFIX: &str = "provider_ops:quota:";
const ADMIN_PROVIDER_OPS_QUOTA_CACHE_TTL_SECS: u64 = 3_600;

pub(super) async fn admin_provider_ops_run_query_quota_action(
    state: &AdminAppState<'_>,
    provider_id: &str,
    provider: &StoredProviderCatalogProvider,
    _endpoints: &[StoredProviderCatalogEndpoint],
    request_override: Option<&Map<String, Value>>,
) -> Value {
    if provider.provider_type != "custom" {
        return admin_provider_ops_action_error(
            "not_supported",
            "query_quota",
            "额度查询仅适用于自定义类型提供商",
            None,
        );
    }

    let script = match provider
        .config
        .as_ref()
        .and_then(|config| config.get("quota_query"))
        .and_then(|config| config.get("script"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(script) => script.to_string(),
        None => {
            return admin_provider_ops_action_error(
                "not_configured",
                "query_quota",
                "未配置额度查询方式",
                None,
            )
        }
    };
    // Test-time overrides (e.g. the in-form test button) replace the whole
    // script and never persist: the stored provider config is only read here.
    let script = request_override
        .and_then(|override_map| override_map.get("script"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&script)
        .to_string();

    let start = std::time::Instant::now();
    let request = match validate_quota_script(&script) {
        Ok(request) => request,
        Err(message) => {
            return admin_provider_ops_action_error(
                "not_configured",
                "query_quota",
                format!("额度查询脚本无效: {message}"),
                None,
            )
        }
    };

    // Resolve `{{apiKey}}` placeholders from the provider's existing Provider
    // Ops credentials; the quota script itself never stores secrets.
    let variables = match quota_script_variables(state, provider, &request).await {
        Ok(variables) => variables,
        Err(message) => {
            return admin_provider_ops_action_error("not_configured", "query_quota", message, None)
        }
    };

    let url = match substitute_quota_placeholders(&request.url, &variables) {
        Ok(url) => url,
        Err(message) => {
            return admin_provider_ops_action_error("not_configured", "query_quota", message, None)
        }
    };
    let url = match url::Url::parse(&url) {
        Ok(parsed) if matches!(parsed.scheme(), "http" | "https") => parsed.to_string(),
        _ => {
            return admin_provider_ops_action_error(
                "not_configured",
                "query_quota",
                "请求地址必须是完整的 http/https URL",
                None,
            )
        }
    };
    let headers = match build_quota_query_headers(&request, &variables) {
        Ok(headers) => headers,
        Err(message) => {
            return admin_provider_ops_action_error("not_configured", "query_quota", message, None)
        }
    };
    let json_body = match &request.body {
        Some(body) => match substitute_quota_body_placeholders(body, &variables) {
            Ok(body) => Some(body),
            Err(message) => {
                return admin_provider_ops_action_error(
                    "not_configured",
                    "query_quota",
                    message,
                    None,
                )
            }
        },
        None => None,
    };
    let method = match reqwest::Method::from_bytes(request.method.as_bytes()) {
        Ok(method) => method,
        Err(_) => reqwest::Method::GET,
    };

    let (status, response_json) = match admin_provider_ops_execute_json_request(
        state,
        &format!("provider-quota-query:{provider_id}"),
        method,
        &url,
        &headers,
        json_body,
        None,
    )
    .await
    {
        Ok(result) => result,
        Err(AdminProviderOpsExecuteJsonError::InvalidJson(_)) => {
            return admin_provider_ops_action_error(
                "parse_error",
                "query_quota",
                "响应不是有效的 JSON",
                Some(start.elapsed().as_millis() as u64),
            );
        }
        Err(AdminProviderOpsExecuteJsonError::Transport(err)) => {
            return admin_provider_ops_action_error(
                "network_error",
                "query_quota",
                admin_provider_ops_network_error_message(&err),
                None,
            );
        }
    };
    let response_time_ms = Some(start.elapsed().as_millis() as u64);

    if status != http::StatusCode::OK {
        return match status {
            http::StatusCode::UNAUTHORIZED | http::StatusCode::FORBIDDEN => {
                admin_provider_ops_action_error(
                    "auth_failed",
                    "query_quota",
                    "认证失败",
                    response_time_ms,
                )
            }
            http::StatusCode::NOT_FOUND => admin_provider_ops_action_error(
                "not_supported",
                "query_quota",
                "接口不存在",
                response_time_ms,
            ),
            http::StatusCode::TOO_MANY_REQUESTS => admin_provider_ops_action_error(
                "rate_limited",
                "query_quota",
                "请求频率限制",
                response_time_ms,
            ),
            _ => admin_provider_ops_action_error(
                "unknown_error",
                "query_quota",
                format!(
                    "HTTP {}: {}",
                    status.as_u16(),
                    status.canonical_reason().unwrap_or("Unknown")
                ),
                response_time_ms,
            ),
        };
    }

    // The sandboxed Rhai engine is CPU-bound (capped at 2s); keep it off the
    // async worker threads.
    let script_for_extraction = script.clone();
    let data = match tokio::task::spawn_blocking(move || {
        run_quota_extractor(&script_for_extraction, &response_json)
    })
    .await
    {
        Ok(Ok(data)) => data,
        Ok(Err(message)) => {
            return admin_provider_ops_action_error(
                "parse_error",
                "query_quota",
                message,
                response_time_ms,
            );
        }
        Err(_) => {
            return admin_provider_ops_action_error(
                "unknown_error",
                "query_quota",
                "额度查询脚本执行被中断",
                response_time_ms,
            );
        }
    };

    let payload = admin_provider_ops_action_response(
        "success",
        "query_quota",
        data.clone(),
        None,
        response_time_ms,
        ADMIN_PROVIDER_OPS_QUOTA_CACHE_TTL_SECS,
    );
    // Test-time overrides (e.g. the in-form test button) must not poison the
    // cached result that the provider summary and accounts views read.
    if request_override.is_none() {
        store_admin_provider_ops_quota_cache(state, provider_id, &payload).await;
        persist_custom_quota_key_snapshots(state, provider_id, &data).await;
    }
    payload
}

/// Project the extractor result onto the shared per-key quota snapshot
/// (`status_snapshot.quota.windows`) that the codex / claude / kiro displays
/// render from, so custom providers reuse the exact same UI.
async fn persist_custom_quota_key_snapshots(
    state: &AdminAppState<'_>,
    provider_id: &str,
    data: &Value,
) {
    let Some(quota) = build_custom_quota_snapshot(data) else {
        return;
    };
    let now_unix_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs());
    let keys = match state
        .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(&provider_id.to_string()))
        .await
    {
        Ok(keys) => keys,
        Err(err) => {
            tracing::warn!(error = ?err, provider_id, "failed to list keys for custom quota snapshot");
            return;
        }
    };
    for key in keys.iter().filter(|key| key.is_active) {
        let update = ProviderCatalogKeyStatusSnapshotUpdate {
            key_id: key.id.clone(),
            status_snapshot_patch: serde_json::json!({ "quota": quota }),
            updated_at_unix_secs: now_unix_secs,
        };
        if let Err(err) = state
            .update_provider_catalog_key_status_snapshot(&update)
            .await
        {
            tracing::warn!(error = ?err, provider_id, key_id = %key.id, "failed to persist custom quota snapshot");
        }
    }
}

/// Extract the trimmed non-empty quota script from a provider config, using
/// the same path the action itself reads.
fn provider_quota_script(config: Option<&Value>) -> Option<String> {
    config
        .and_then(|config| config.get("quota_query"))
        .and_then(|config| config.get("script"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// Keep the quota display in sync when a provider write changes the script:
/// the cached payload and persisted key snapshots are cleared, then the query
/// re-runs in the background so the KV cache and per-key snapshots reflect the
/// new script without waiting for the hourly TTL. Removing the script just
/// leaves the windows cleared.
pub(crate) fn sync_custom_quota_display_after_write(
    state: &AdminAppState<'_>,
    existing_config: Option<&Value>,
    updated: &StoredProviderCatalogProvider,
) {
    if updated.provider_type != "custom" {
        return;
    }
    if provider_quota_script(existing_config) == provider_quota_script(updated.config.as_ref()) {
        return;
    }
    let provider_id = updated.id.clone();
    let updated = updated.clone();
    let app = state.cloned_app();
    tokio::spawn(async move {
        let state = AdminAppState::new(&app);
        let cache_key = format!("{ADMIN_PROVIDER_OPS_QUOTA_CACHE_PREFIX}{provider_id}");
        if let Err(err) = state.runtime_state().kv_delete(&cache_key).await {
            tracing::warn!(
                error = ?err,
                provider_id,
                "failed to invalidate custom quota cache after script change"
            );
        }
        // Old windows must not outlive the script change: clear first, then a
        // successful run repopulates. A failed run therefore shows no quota
        // instead of silently rendering the previous script's numbers.
        clear_custom_quota_key_snapshots(&state, &provider_id).await;
        if provider_quota_script(updated.config.as_ref()).is_none() {
            return;
        }
        let payload =
            admin_provider_ops_run_query_quota_action(&state, &provider_id, &updated, &[], None)
                .await;
        match payload.get("status").and_then(Value::as_str) {
            Some("success") => {
                tracing::debug!(provider_id, "custom quota refreshed after script change");
            }
            other => {
                let message = payload
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                tracing::warn!(
                    provider_id,
                    status = other.unwrap_or("unknown"),
                    message,
                    "custom quota refresh after script change did not succeed"
                );
            }
        }
    });
}

/// Remove the persisted quota windows from every active key (script removed).
/// The snapshot patch is a top-level jsonb merge, so `quota: null` blanks the
/// quota entry that the displays read.
async fn clear_custom_quota_key_snapshots(state: &AdminAppState<'_>, provider_id: &str) {
    let keys = match state
        .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(&provider_id.to_string()))
        .await
    {
        Ok(keys) => keys,
        Err(err) => {
            tracing::warn!(error = ?err, provider_id, "failed to list keys to clear custom quota snapshot");
            return;
        }
    };
    for key in keys.iter().filter(|key| key.is_active) {
        let update = ProviderCatalogKeyStatusSnapshotUpdate {
            key_id: key.id.clone(),
            status_snapshot_patch: serde_json::json!({ "quota": Value::Null }),
            updated_at_unix_secs: None,
        };
        if let Err(err) = state
            .update_provider_catalog_key_status_snapshot(&update)
            .await
        {
            tracing::warn!(
                error = ?err,
                provider_id,
                key_id = %key.id,
                "failed to clear custom quota snapshot"
            );
        }
    }
}

/// Build the shared `status_snapshot.quota` value from an extractor result.
/// Window codes use the shared vocabulary (`5h` / `weekly` / `monthly`) so the
/// existing codex display derives the 周 / 5H labels itself.
fn build_custom_quota_snapshot(data: &Value) -> Option<Value> {
    let periods = data.get("periods")?.as_array()?;
    let now_unix_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs());
    let mut windows = Vec::new();
    for period in periods.iter().take(8) {
        let Some(key) = period
            .get("key")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let code = match key.to_ascii_lowercase().as_str() {
            "week" | "weekly" => "weekly".to_string(),
            "month" | "monthly" => "monthly".to_string(),
            other => other.to_string(),
        };
        let used = period.get("used").and_then(Value::as_f64);
        let limit = period.get("limit").and_then(Value::as_f64);
        let used_ratio = match (used, limit) {
            (Some(used), Some(limit)) if limit > 0.0 && used.is_finite() => {
                Some((used / limit).clamp(0.0, 1.0))
            }
            _ => None,
        };
        let reset_at = period
            .get("reset_at")
            .and_then(Value::as_str)
            .and_then(|raw| chrono::DateTime::parse_from_rfc3339(raw).ok())
            .map(|date| date.timestamp());
        let mut window = serde_json::Map::new();
        window.insert("code".to_string(), serde_json::json!(code));
        window.insert("label".to_string(), serde_json::json!(key));
        window.insert("scope".to_string(), serde_json::json!("account"));
        if let Some(used_ratio) = used_ratio {
            window.insert("used_ratio".to_string(), serde_json::json!(used_ratio));
        }
        if let Some(reset_at) = reset_at {
            window.insert("reset_at".to_string(), serde_json::json!(reset_at));
        }
        windows.push(Value::Object(window));
    }
    if windows.is_empty() {
        return None;
    }
    let is_valid = data.get("is_valid").and_then(Value::as_bool);
    let quota = serde_json::json!({
        "provider_type": "custom",
        "code": if is_valid == Some(false) { "exhausted" } else { "ok" },
        "exhausted": is_valid == Some(false),
        "label": data.get("plan_name").and_then(Value::as_str),
        "reason": data.get("invalid_message").and_then(Value::as_str),
        "updated_at": now_unix_secs,
        "windows": windows,
    });
    Some(quota)
}

/// Collect placeholder variables for `{{apiKey}}`. Credentials are only
/// resolved (and their absence reported) when the script actually references
/// them. Provider Ops credentials take precedence; when they are absent the
/// provider's first usable pool key is used, so providers with keys work
/// without any extra Provider Ops setup.
async fn quota_script_variables(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    request: &QuotaScriptRequest,
) -> Result<BTreeMap<String, String>, String> {
    if !request.references_api_key() {
        return Ok(BTreeMap::new());
    }
    let ops_api_key = match admin_provider_ops_credential_snapshot(state, provider).await {
        Ok(snapshot) => snapshot
            .credentials
            .get("api_key")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned),
        Err(_) => None,
    };
    let api_key = match ops_api_key {
        Some(key) => Some(key),
        None => first_provider_pool_api_key(state, provider).await?,
    };
    let Some(api_key) = api_key else {
        return Err(
            "无法解析 {{apiKey}} 占位符：请先在扩展操作中保存凭据，或为提供商添加可用的 API Key"
                .to_string(),
        );
    };
    let mut variables = BTreeMap::new();
    variables.insert("apiKey".to_string(), api_key.clone());
    variables.insert("api_key".to_string(), api_key);
    Ok(variables)
}

/// Decrypt the provider's first active pool key, skipping keys without a
/// usable plaintext credential.
async fn first_provider_pool_api_key(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
) -> Result<Option<String>, String> {
    let keys = state
        .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(&provider.id))
        .await
        .map_err(|_| "读取提供商密钥失败".to_string())?;
    let mut keys: Vec<_> = keys.into_iter().filter(|key| key.is_active).collect();
    keys.sort_by_key(|key| key.internal_priority);
    for key in keys {
        if key
            .encrypted_api_key
            .as_deref()
            .map(str::trim)
            .is_none_or(str::is_empty)
        {
            continue;
        }
        if let Some(plaintext) = state
            .app()
            .decrypt_provider_catalog_key_api_key(&key)
            .ok()
            .flatten()
        {
            if !plaintext.trim().is_empty() && plaintext != "__placeholder__" {
                return Ok(Some(plaintext));
            }
        }
    }
    Ok(None)
}

fn substitute_quota_placeholders(
    value: &str,
    variables: &BTreeMap<String, String>,
) -> Result<String, String> {
    let mut result = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("{{") {
        result.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                let key = after[..end].trim();
                match variables.get(key) {
                    Some(replacement) => result.push_str(replacement),
                    None => {
                        return Err(format!(
                            "占位符 {{{{{key}}}}} 无法解析，请检查端点地址与凭据配置"
                        ))
                    }
                }
                rest = &after[end + 2..];
            }
            None => {
                result.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    result.push_str(rest);
    Ok(result)
}

fn build_quota_query_headers(
    request: &QuotaScriptRequest,
    variables: &BTreeMap<String, String>,
) -> Result<reqwest::header::HeaderMap, String> {
    let mut headers = reqwest::header::HeaderMap::new();
    for (name, value) in &request.headers {
        let resolved = substitute_quota_placeholders(value, variables)?;
        let header_name = reqwest::header::HeaderName::from_bytes(name.trim().as_bytes())
            .map_err(|_| format!("请求头名称无效: {name}"))?;
        let header_value = reqwest::header::HeaderValue::from_str(&resolved)
            .map_err(|_| format!("请求头 {name} 的值无效"))?;
        headers.insert(header_name, header_value);
    }
    Ok(headers)
}

/// Substitute `{{apiKey}}` placeholders inside a JSON body, walking string
/// leaves only so the JSON structure and escaping stay intact.
fn substitute_quota_body_placeholders(
    body: &Value,
    variables: &BTreeMap<String, String>,
) -> Result<Value, String> {
    match body {
        Value::String(text) => Ok(Value::String(substitute_quota_placeholders(
            text, variables,
        )?)),
        Value::Array(items) => items
            .iter()
            .map(|item| substitute_quota_body_placeholders(item, variables))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Value::Object(map) => map
            .iter()
            .map(|(key, value)| {
                substitute_quota_body_placeholders(value, variables)
                    .map(|resolved| (key.clone(), resolved))
            })
            .collect::<Result<serde_json::Map<String, _>, _>>()
            .map(Value::Object),
        other => Ok(other.clone()),
    }
}

async fn store_admin_provider_ops_quota_cache(
    state: &AdminAppState<'_>,
    provider_id: &str,
    payload: &Value,
) {
    if payload.get("status").and_then(Value::as_str) != Some("success") {
        return;
    }
    let Ok(serialized) = serde_json::to_string(payload) else {
        return;
    };
    if let Err(err) = state
        .runtime_state()
        .kv_set(
            &format!("{ADMIN_PROVIDER_OPS_QUOTA_CACHE_PREFIX}{provider_id}"),
            serialized,
            Some(Duration::from_secs(ADMIN_PROVIDER_OPS_QUOTA_CACHE_TTL_SECS)),
        )
        .await
    {
        tracing::warn!(error = %err, provider_id, "failed to store provider ops quota cache");
    }
}

fn admin_provider_ops_network_error_message(error: &str) -> String {
    let normalized = error.trim();
    let lower = normalized.to_ascii_lowercase();
    if lower.contains("timeout") || normalized.contains("超时") {
        return "请求超时".to_string();
    }
    "网络错误".to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        build_quota_query_headers, substitute_quota_body_placeholders,
        substitute_quota_placeholders,
    };
    use aether_admin::provider::ops::quota_script::QuotaScriptRequest;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn variables() -> BTreeMap<String, String> {
        let mut variables = BTreeMap::new();
        variables.insert("apiKey".to_string(), "sk-secret".to_string());
        variables
    }

    #[test]
    fn placeholder_substitution_replaces_known_and_rejects_unknown() {
        let variables = variables();
        assert_eq!(
            substitute_quota_placeholders(
                "https://api.example.test/user/balance?key={{apiKey}}",
                &variables
            )
            .expect("substitute"),
            "https://api.example.test/user/balance?key=sk-secret"
        );
        assert_eq!(
            substitute_quota_placeholders("Bearer {{apiKey}}", &variables).expect("substitute"),
            "Bearer sk-secret"
        );
        let error = substitute_quota_placeholders("{{missing}}", &variables)
            .expect_err("unknown placeholder should fail");
        assert!(error.contains("missing"), "unexpected error: {error}");
    }

    #[test]
    fn headers_substitute_placeholders() {
        let request = QuotaScriptRequest {
            url: "https://api.example.test/quota".to_string(),
            method: "GET".to_string(),
            headers: [
                ("Authorization".to_string(), "Bearer {{apiKey}}".to_string()),
                ("X-Scope".to_string(), "all".to_string()),
            ]
            .into_iter()
            .collect(),
            body: None,
        };
        let headers = build_quota_query_headers(&request, &variables()).expect("headers");
        assert_eq!(
            headers.get("Authorization").and_then(|v| v.to_str().ok()),
            Some("Bearer sk-secret")
        );
        assert_eq!(
            headers.get("X-Scope").and_then(|v| v.to_str().ok()),
            Some("all")
        );
    }

    #[test]
    fn body_placeholders_substitute_inside_json_structure() {
        let body = json!({
            "query": "quota { remaining }",
            "credentials": { "token": "{{apiKey}}" },
            "aliases": ["{{apiKey}}", 3, true, null],
            "count": 2
        });
        let resolved = substitute_quota_body_placeholders(&body, &variables()).expect("body");
        assert_eq!(
            resolved,
            json!({
                "query": "quota { remaining }",
                "credentials": { "token": "sk-secret" },
                "aliases": ["sk-secret", 3, true, json!(null)],
                "count": 2
            })
        );

        // Unknown placeholders surface the same error as headers and URLs.
        let error =
            substitute_quota_body_placeholders(&json!({ "key": "{{missing}}" }), &variables())
                .expect_err("unknown placeholder should fail");
        assert!(error.contains("missing"), "unexpected error: {error}");
    }
}
