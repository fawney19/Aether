use super::actions::admin_provider_ops_local_action_response;
use crate::handlers::admin::request::AdminAppState;
use crate::task_runtime::{spawn_fire_and_forget, TASK_KEY_PROVIDER_BALANCE_REFRESH};
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::time::Duration;
use tokio::sync::{Mutex, Semaphore};
use tracing::{debug, warn};

const ADMIN_PROVIDER_OPS_BALANCE_CACHE_PREFIX: &str = "provider_ops:balance:";
const ADMIN_PROVIDER_OPS_BALANCE_REFRESH_PREFIX: &str = "provider_ops:balance_refresh:";
const ADMIN_PROVIDER_OPS_BALANCE_CACHE_TTL_SECS: u64 = 86_400;
const ADMIN_PROVIDER_OPS_BALANCE_AUTH_FAILED_CACHE_TTL_SECS: u64 = 60;
const ADMIN_PROVIDER_OPS_BALANCE_REFRESH_CONCURRENCY: usize = 3;

static ADMIN_PROVIDER_OPS_BALANCE_REFRESH_SEMAPHORE: std::sync::LazyLock<Semaphore> =
    std::sync::LazyLock::new(|| Semaphore::new(ADMIN_PROVIDER_OPS_BALANCE_REFRESH_CONCURRENCY));
static ADMIN_PROVIDER_OPS_REFRESHING_PROVIDERS: std::sync::LazyLock<Mutex<HashSet<String>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashSet::new()));

#[derive(Debug)]
pub(super) enum AdminProviderOpsBalanceCacheLookup {
    Hit(Value),
    Miss,
    Unavailable,
}

pub(super) fn admin_provider_ops_batch_balance_concurrency() -> usize {
    std::env::var("BATCH_BALANCE_CONCURRENCY")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .map(|value| value.max(1))
        .unwrap_or(3)
}

pub(super) fn admin_provider_ops_pending_balance_response(message: &str) -> Value {
    json!({
        "status": "pending",
        "action_type": "query_balance",
        "data": Value::Null,
        "message": message,
        "executed_at": chrono::Utc::now()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "response_time_ms": Value::Null,
        "cache_ttl_seconds": 0,
    })
}

pub(super) async fn read_admin_provider_ops_balance_cache(
    state: &AdminAppState<'_>,
    provider_id: &str,
) -> AdminProviderOpsBalanceCacheLookup {
    let raw_key = format!("{ADMIN_PROVIDER_OPS_BALANCE_CACHE_PREFIX}{provider_id}");
    let raw = match state.runtime_state().kv_get(&raw_key).await {
        Ok(raw) => raw,
        Err(err) => {
            warn!(error = %err, provider_id, "failed to read provider ops balance runtime cache");
            return AdminProviderOpsBalanceCacheLookup::Unavailable;
        }
    };
    let Some(raw) = raw else {
        return AdminProviderOpsBalanceCacheLookup::Miss;
    };
    match serde_json::from_str::<Value>(&raw) {
        Ok(payload) => AdminProviderOpsBalanceCacheLookup::Hit(payload),
        Err(err) => {
            warn!(error = %err, provider_id, "failed to parse provider ops balance cache payload");
            AdminProviderOpsBalanceCacheLookup::Miss
        }
    }
}

pub(crate) async fn store_admin_provider_ops_balance_cache(
    state: &AdminAppState<'_>,
    provider_id: &str,
    payload: &Value,
) {
    let Some(projected) = project_admin_provider_ops_balance_cache_payload(payload) else {
        return;
    };
    let Some(ttl_seconds) = balance_cache_ttl_seconds(&projected) else {
        return;
    };
    let serialized = match serde_json::to_string(&projected) {
        Ok(serialized) => serialized,
        Err(err) => {
            warn!(
                error = %err,
                provider_id,
                "failed to serialize provider ops balance payload"
            );
            return;
        }
    };
    if let Err(err) = state
        .runtime_state()
        .kv_set(
            &format!("{ADMIN_PROVIDER_OPS_BALANCE_CACHE_PREFIX}{provider_id}"),
            serialized,
            Some(Duration::from_secs(ttl_seconds)),
        )
        .await
    {
        warn!(error = %err, provider_id, "failed to store provider ops balance cache");
    }
}

pub(super) async fn clear_admin_provider_ops_balance_cache(
    state: &AdminAppState<'_>,
    provider_id: &str,
) {
    if let Err(err) = state
        .runtime_state()
        .kv_delete(&format!(
            "{ADMIN_PROVIDER_OPS_BALANCE_CACHE_PREFIX}{provider_id}"
        ))
        .await
    {
        warn!(error = %err, provider_id, "failed to clear provider ops balance cache");
    }
}

pub(super) async fn spawn_admin_provider_ops_balance_refresh(
    state: &AdminAppState<'_>,
    provider_id: &str,
) {
    let refresh_key = admin_provider_ops_balance_refresh_key(state, provider_id);
    let mut guard = ADMIN_PROVIDER_OPS_REFRESHING_PROVIDERS.lock().await;
    if !guard.insert(refresh_key.clone()) {
        debug!(provider_id, "provider ops balance refresh already running");
        return;
    }
    drop(guard);

    let app = state.cloned_app();
    let provider_id = provider_id.to_string();
    spawn_fire_and_forget(TASK_KEY_PROVIDER_BALANCE_REFRESH, async move {
        let permit = match tokio::time::timeout(
            Duration::from_secs(5),
            ADMIN_PROVIDER_OPS_BALANCE_REFRESH_SEMAPHORE.acquire(),
        )
        .await
        {
            Ok(Ok(permit)) => permit,
            Ok(Err(err)) => {
                warn!(
                    provider_id = %provider_id,
                    error = %err,
                    "provider ops balance refresh semaphore closed"
                );
                finish_refresh_provider(&refresh_key).await;
                return;
            }
            Err(_) => {
                debug!(provider_id = %provider_id, "provider ops balance refresh skipped by concurrency limit");
                finish_refresh_provider(&refresh_key).await;
                return;
            }
        };

        let admin_state = AdminAppState::new(&app);
        let provider_ids = [provider_id.clone()];
        let providers = match admin_state
            .read_provider_catalog_providers_by_ids(&provider_ids)
            .await
        {
            Ok(providers) => providers,
            Err(err) => {
                warn!(
                    provider_id = %provider_id,
                    error = ?err,
                    "failed to load provider for balance refresh"
                );
                drop(permit);
                finish_refresh_provider(&refresh_key).await;
                return;
            }
        };
        let provider = providers.first();
        let endpoints = if provider.is_some() {
            match admin_state
                .list_provider_catalog_endpoints_by_provider_ids(&provider_ids)
                .await
            {
                Ok(endpoints) => endpoints,
                Err(err) => {
                    warn!(
                        provider_id = %provider_id,
                        error = ?err,
                        "failed to load endpoints for balance refresh"
                    );
                    drop(permit);
                    finish_refresh_provider(&refresh_key).await;
                    return;
                }
            }
        } else {
            Vec::new()
        };

        let payload = admin_provider_ops_local_action_response(
            &admin_state,
            &provider_id,
            provider,
            &endpoints,
            "query_balance",
            None,
        )
        .await;
        store_admin_provider_ops_balance_cache(&admin_state, &provider_id, &payload).await;
        drop(permit);
        finish_refresh_provider(&refresh_key).await;
    });
}

fn balance_cache_ttl_seconds(payload: &Value) -> Option<u64> {
    match payload
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "success" | "auth_expired" => Some(ADMIN_PROVIDER_OPS_BALANCE_CACHE_TTL_SECS),
        "auth_failed" => Some(ADMIN_PROVIDER_OPS_BALANCE_AUTH_FAILED_CACHE_TTL_SECS),
        _ => None,
    }
}

const BALANCE_CACHE_EXTRA_NUMERIC_FIELDS: &[&str] = &[
    "balance",
    "points",
    "active_subscriptions",
    "total_used_usd",
    "normal_balance",
    "subscription_balance",
    "charity_balance",
    "pay_as_you_go_balance",
    "daily_limit",
    "weekly_limit",
    "weekly_spent",
    "daily_spent",
    "daily_used_quota",
    "daily_quota_limit",
    "daily_remaining_quota",
];

const BALANCE_CACHE_EXTRA_STRING_FIELDS: &[&str] = &[
    "plan_name",
    "subscription_status",
    "status",
    "group_name",
    "effective_start_date",
    "effective_end_date",
];

const BALANCE_CACHE_EXTRA_BOOL_FIELDS: &[&str] = &["checkin_success", "cookie_expired"];

const BALANCE_CACHE_EXTRA_NESTED_FIELDS: &[&str] = &[
    "five_hour_limit",
    "weekly_limit",
    "month_stats",
    "subscriptions",
];
const BALANCE_CACHE_LIMIT_FIELDS: &[&str] = &["limit", "used", "remaining", "resets_at"];
const BALANCE_CACHE_MONTH_STATS_FIELDS: &[&str] = &[
    "total_input_tokens",
    "total_output_tokens",
    "total_quota",
    "total_requests",
];

fn project_admin_provider_ops_balance_cache_payload(payload: &Value) -> Option<Value> {
    let source = payload.as_object()?;
    let status = source.get("status").and_then(Value::as_str)?.trim();
    if !matches!(status, "success" | "auth_expired" | "auth_failed") {
        return None;
    }
    if source.get("action_type").and_then(Value::as_str) != Some("query_balance") {
        return None;
    }

    let mut projected = Map::new();
    projected.insert("status".to_string(), Value::String(status.to_string()));
    projected.insert(
        "action_type".to_string(),
        Value::String("query_balance".to_string()),
    );

    let data = match source.get("data") {
        Some(Value::Null) | None => Value::Null,
        Some(value) => project_admin_provider_ops_balance_data(value)?,
    };
    projected.insert("data".to_string(), data);
    projected.insert(
        "message".to_string(),
        match status {
            "auth_failed" => Value::String("认证失败".to_string()),
            "auth_expired" => Value::String("认证已过期".to_string()),
            _ => Value::Null,
        },
    );
    if let Some(value) = source
        .get("executed_at")
        .and_then(project_admin_provider_ops_safe_string)
    {
        projected.insert("executed_at".to_string(), Value::String(value));
    }
    if let Some(value) = source
        .get("response_time_ms")
        .and_then(project_admin_provider_ops_finite_number)
    {
        projected.insert("response_time_ms".to_string(), value);
    }
    projected.insert(
        "cache_ttl_seconds".to_string(),
        Value::from(if status == "auth_failed" {
            ADMIN_PROVIDER_OPS_BALANCE_AUTH_FAILED_CACHE_TTL_SECS
        } else {
            ADMIN_PROVIDER_OPS_BALANCE_CACHE_TTL_SECS
        }),
    );
    Some(Value::Object(projected))
}

fn project_admin_provider_ops_balance_data(value: &Value) -> Option<Value> {
    let source = value.as_object()?;
    let mut projected = Map::new();
    for field in ["total_granted", "total_used", "total_available"] {
        if let Some(value) = source.get(field) {
            projected.insert(
                field.to_string(),
                project_admin_provider_ops_finite_number_or_null(value)?,
            );
        }
    }
    if let Some(value) = source.get("expires_at") {
        projected.insert(
            "expires_at".to_string(),
            project_admin_provider_ops_finite_number_or_null(value)?,
        );
    }
    if let Some(value) = source.get("currency") {
        let currency = project_admin_provider_ops_safe_string(value)?;
        if currency.len() > 32
            || !currency
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/'))
        {
            return None;
        }
        projected.insert("currency".to_string(), Value::String(currency));
    }
    if let Some(extra) = source.get("extra") {
        projected.insert(
            "extra".to_string(),
            project_admin_provider_ops_balance_extra(extra)?,
        );
    }
    Some(Value::Object(projected))
}

fn project_admin_provider_ops_balance_extra(value: &Value) -> Option<Value> {
    let source = value.as_object()?;
    let mut projected = Map::new();
    for (field, value) in source {
        let projected_value = if BALANCE_CACHE_EXTRA_NUMERIC_FIELDS.contains(&field.as_str())
            && project_admin_provider_ops_finite_number(value).is_some()
        {
            project_admin_provider_ops_finite_number(value)
        } else if BALANCE_CACHE_EXTRA_STRING_FIELDS.contains(&field.as_str()) {
            project_admin_provider_ops_safe_string(value).map(Value::String)
        } else if BALANCE_CACHE_EXTRA_BOOL_FIELDS.contains(&field.as_str()) {
            value.as_bool().map(Value::Bool)
        } else if BALANCE_CACHE_EXTRA_NESTED_FIELDS.contains(&field.as_str()) {
            project_admin_provider_ops_balance_extra_nested(field, value)
        } else if matches!(
            field.as_str(),
            "weekly_resets_at" | "daily_resets_at" | "resets_at"
        ) {
            project_admin_provider_ops_finite_number_or_safe_string(value)
        } else if matches!(field.as_str(), "checkin_message" | "cookie_expired_message") {
            project_admin_provider_ops_safe_string(value).map(Value::String)
        } else {
            None
        };
        if let Some(projected_value) = projected_value {
            projected.insert(field.clone(), projected_value);
        }
    }
    Some(Value::Object(projected))
}

fn project_admin_provider_ops_balance_extra_nested(field: &str, value: &Value) -> Option<Value> {
    if field == "subscriptions" {
        let items = value.as_array()?;
        return Some(Value::Array(
            items
                .iter()
                .take(128)
                .filter_map(project_admin_provider_ops_subscription)
                .collect(),
        ));
    }
    let source = value.as_object()?;
    let mut projected = Map::new();
    let allowed = if field == "month_stats" {
        BALANCE_CACHE_MONTH_STATS_FIELDS
    } else {
        BALANCE_CACHE_LIMIT_FIELDS
    };
    for key in allowed {
        if let Some(value) = source.get(*key) {
            let projected_value = if *key == "resets_at" {
                project_admin_provider_ops_finite_number_or_safe_string(value)
            } else {
                project_admin_provider_ops_finite_number(value)
            };
            if let Some(projected_value) = projected_value {
                projected.insert((*key).to_string(), projected_value);
            }
        }
    }
    Some(Value::Object(projected))
}

fn project_admin_provider_ops_subscription(value: &Value) -> Option<Value> {
    let source = value.as_object()?;
    let mut projected = Map::new();
    for field in ["group_name", "status"] {
        if let Some(value) = source.get(field) {
            projected.insert(
                field.to_string(),
                Value::String(project_admin_provider_ops_safe_string(value)?),
            );
        }
    }
    for field in [
        "daily_used_usd",
        "daily_limit_usd",
        "weekly_used_usd",
        "weekly_limit_usd",
        "monthly_used_usd",
        "monthly_limit_usd",
    ] {
        if let Some(value) = source.get(field) {
            if let Some(value) = project_admin_provider_ops_finite_number(value) {
                projected.insert(field.to_string(), value);
            }
        }
    }
    if let Some(value) = source.get("expires_at") {
        if let Some(value) = project_admin_provider_ops_finite_number_or_safe_string(value) {
            projected.insert("expires_at".to_string(), value);
        }
    }
    Some(Value::Object(projected))
}

fn project_admin_provider_ops_finite_number(value: &Value) -> Option<Value> {
    if let Some(number) = value.as_f64() {
        return number.is_finite().then(|| value.clone());
    }
    let number = value.as_str()?.trim().parse::<f64>().ok()?;
    number.is_finite().then(|| Value::from(number))
}

fn project_admin_provider_ops_finite_number_or_null(value: &Value) -> Option<Value> {
    if value.is_null() {
        Some(Value::Null)
    } else {
        project_admin_provider_ops_finite_number(value)
    }
}

fn project_admin_provider_ops_finite_number_or_safe_string(value: &Value) -> Option<Value> {
    project_admin_provider_ops_finite_number(value)
        .or_else(|| project_admin_provider_ops_safe_string(value).map(Value::String))
}

fn project_admin_provider_ops_safe_string(value: &Value) -> Option<String> {
    let value = value.as_str()?.trim();
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return None;
    }
    let lower = value.to_ascii_lowercase();
    if [
        "authorization",
        "bearer ",
        "api_key",
        "apikey",
        "access_token",
        "refresh_token",
        "password",
        "cookie",
        "session",
        "secret",
        "token=",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
    {
        return None;
    }
    Some(value.to_string())
}

fn admin_provider_ops_balance_refresh_key(state: &AdminAppState<'_>, provider_id: &str) -> String {
    let raw_key = format!("{ADMIN_PROVIDER_OPS_BALANCE_REFRESH_PREFIX}{provider_id}");
    format!(
        "{:p}:{}",
        state.app(),
        state.runtime_state().namespace_key(raw_key.as_str())
    )
}

async fn finish_refresh_provider(refresh_key: &str) {
    ADMIN_PROVIDER_OPS_REFRESHING_PROVIDERS
        .lock()
        .await
        .remove(refresh_key);
}

fn admin_provider_ops_action_response(
    total_available: f64,
    extra: serde_json::Map<String, Value>,
) -> Value {
    json!({
        "status": "success",
        "action_type": "query_balance",
        "data": {
            "total_granted": Value::Null,
            "total_used": Value::Null,
            "total_available": total_available,
            "expires_at": Value::Null,
            "currency": "USD",
            "extra": extra,
        },
        "message": Value::Null,
        "executed_at": chrono::Utc::now()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "response_time_ms": Value::Null,
        "cache_ttl_seconds": ADMIN_PROVIDER_OPS_BALANCE_CACHE_TTL_SECS,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        admin_provider_ops_pending_balance_response, balance_cache_ttl_seconds,
        project_admin_provider_ops_balance_cache_payload,
    };
    use serde_json::json;

    #[test]
    fn pending_balance_response_uses_pending_status() {
        let payload = admin_provider_ops_pending_balance_response("余额数据加载中，请稍后刷新");
        assert_eq!(payload["status"], json!("pending"));
        assert_eq!(payload["action_type"], json!("query_balance"));
    }

    #[test]
    fn balance_cache_ttl_matches_status_contract() {
        assert_eq!(
            balance_cache_ttl_seconds(&json!({ "status": "success" })),
            Some(86400)
        );
        assert_eq!(
            balance_cache_ttl_seconds(&json!({ "status": "auth_expired" })),
            Some(86400)
        );
        assert_eq!(
            balance_cache_ttl_seconds(&json!({ "status": "auth_failed" })),
            Some(60)
        );
        assert_eq!(
            balance_cache_ttl_seconds(&json!({ "status": "network_error" })),
            None
        );
    }

    #[test]
    fn balance_cache_projection_drops_untrusted_messages_and_fields() {
        let payload = json!({
            "status": "auth_failed",
            "action_type": "query_balance",
            "message": "authorization=Bearer upstream-secret",
            "data": {
                "total_available": 1.25,
                "currency": "USD",
                "extra": {
                    "balance": 1.0,
                    "access_token": "upstream-secret",
                    "today_stats": {"private_note": "upstream-secret"},
                    "checkin_message": "签到失败"
                }
            },
            "cache_ttl_seconds": 999999
        });
        let projected = project_admin_provider_ops_balance_cache_payload(&payload)
            .expect("known balance payload should project");
        assert_eq!(projected["message"], json!("认证失败"));
        assert_eq!(projected["data"]["extra"]["balance"], json!(1.0));
        assert!(projected.to_string().find("upstream-secret").is_none());
        assert!(projected["data"]["extra"].get("access_token").is_none());
        assert!(projected["data"]["extra"].get("today_stats").is_none());
        assert_eq!(projected["cache_ttl_seconds"], json!(60));
    }

    #[test]
    fn balance_cache_projection_keeps_sub2api_subscription_allowlist() {
        let payload = json!({
            "status": "success",
            "action_type": "query_balance",
            "data": {
                "total_available": 8.5,
                "currency": "USD",
                "extra": {
                    "subscriptions": [{
                        "group_name": "default",
                        "status": "active",
                        "monthly_used_usd": 1.2,
                        "private_token": "must-drop"
                    }]
                }
            }
        });
        let projected = project_admin_provider_ops_balance_cache_payload(&payload)
            .expect("known balance payload should project");
        assert_eq!(
            projected["data"]["extra"]["subscriptions"][0]["group_name"],
            json!("default")
        );
        assert_eq!(
            projected["data"]["extra"]["subscriptions"][0]["monthly_used_usd"],
            json!(1.2)
        );
        assert!(projected["data"]["extra"]["subscriptions"][0]
            .get("private_token")
            .is_none());
    }
}
