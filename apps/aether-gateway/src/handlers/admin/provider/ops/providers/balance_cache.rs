use super::actions::admin_provider_ops_local_action_response;
use crate::handlers::admin::request::AdminAppState;
use crate::task_runtime::{spawn_fire_and_forget, TASK_KEY_PROVIDER_BALANCE_REFRESH};
use serde_json::{json, Value};
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
    let Some(ttl_seconds) = balance_cache_ttl_seconds(payload) else {
        return;
    };
    let serialized = match serde_json::to_string(payload) {
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
    use super::{admin_provider_ops_pending_balance_response, balance_cache_ttl_seconds};
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
}
