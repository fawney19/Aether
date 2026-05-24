use std::collections::HashMap;

use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogProvider,
};
use futures_util::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::warn;

use crate::admin_api::{
    admin_provider_ops_local_action_response, store_admin_provider_ops_balance_cache, AdminAppState,
};
use crate::important_notification::{
    important_notification_dispatch_ready_for_item, send_important_notification_for_item,
    ImportantNotification, PROVIDER_QUOTA_ALERT_ITEM_KEY,
};
use crate::{AppState, GatewayError};

use super::PROVIDER_QUOTA_ALERT_CONCURRENCY;

const PROVIDER_QUOTA_ALERT_STATE_PREFIX: &str = "provider_ops:quota_alert:";
const PROVIDER_QUOTA_ALERT_DEFAULT_FETCH_INTERVAL_SECS: u64 = 30;
const PROVIDER_QUOTA_ALERT_MIN_FETCH_INTERVAL_SECS: u64 = 30;
const PROVIDER_QUOTA_ALERT_REPEAT_COOLDOWN_SECS: u64 = 24 * 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProviderQuotaAlertRunSummary {
    pub(crate) checked: usize,
    pub(crate) alerted: usize,
    pub(crate) skipped: usize,
    pub(crate) failed: usize,
}

#[derive(Debug, Clone)]
struct ProviderQuotaAlertTarget {
    provider: StoredProviderCatalogProvider,
    config: ProviderQuotaAlertConfig,
}

#[derive(Debug, Clone, Copy)]
struct ProviderQuotaAlertConfig {
    enabled: bool,
    threshold_amount: f64,
    fetch_interval_seconds: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ProviderQuotaAlertRuntimeState {
    #[serde(default)]
    last_checked_at: u64,
    #[serde(default)]
    last_available: Option<f64>,
    #[serde(default)]
    below_threshold: bool,
    #[serde(default)]
    last_notified_at: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderQuotaAlertStatus {
    Checked,
    Alerted,
    Skipped,
    Failed,
}

pub(crate) async fn perform_provider_quota_alert_once(
    state: &AppState,
) -> Result<ProviderQuotaAlertRunSummary, GatewayError> {
    if !state.has_provider_catalog_data_reader() {
        return Ok(ProviderQuotaAlertRunSummary {
            checked: 0,
            alerted: 0,
            skipped: 0,
            failed: 0,
        });
    }
    if !important_notification_dispatch_ready_for_item(state, PROVIDER_QUOTA_ALERT_ITEM_KEY).await?
    {
        return Ok(ProviderQuotaAlertRunSummary {
            checked: 0,
            alerted: 0,
            skipped: 0,
            failed: 0,
        });
    }

    let now_unix_secs = now_unix_secs();
    let targets = select_provider_quota_alert_targets(state, now_unix_secs).await?;
    if targets.is_empty() {
        return Ok(ProviderQuotaAlertRunSummary {
            checked: 0,
            alerted: 0,
            skipped: 0,
            failed: 0,
        });
    }

    let provider_ids = targets
        .iter()
        .map(|target| target.provider.id.clone())
        .collect::<Vec<_>>();
    let mut endpoints_by_provider = HashMap::<String, Vec<StoredProviderCatalogEndpoint>>::new();
    for endpoint in state
        .list_provider_catalog_endpoints_by_provider_ids(&provider_ids)
        .await?
    {
        endpoints_by_provider
            .entry(endpoint.provider_id.clone())
            .or_default()
            .push(endpoint);
    }

    let mut results = stream::iter(targets.into_iter().map(|target| {
        let state = state.clone();
        let provider_id = target.provider.id.clone();
        let endpoints = endpoints_by_provider
            .get(&provider_id)
            .cloned()
            .unwrap_or_default();
        async move { run_provider_quota_alert_for_provider(&state, target, endpoints).await }
    }))
    .buffer_unordered(PROVIDER_QUOTA_ALERT_CONCURRENCY);

    let mut summary = ProviderQuotaAlertRunSummary {
        checked: 0,
        alerted: 0,
        skipped: 0,
        failed: 0,
    };
    while let Some(status) = results.next().await {
        match status {
            ProviderQuotaAlertStatus::Checked => summary.checked += 1,
            ProviderQuotaAlertStatus::Alerted => {
                summary.checked += 1;
                summary.alerted += 1;
            }
            ProviderQuotaAlertStatus::Skipped => summary.skipped += 1,
            ProviderQuotaAlertStatus::Failed => summary.failed += 1,
        }
    }

    Ok(summary)
}

async fn select_provider_quota_alert_targets(
    state: &AppState,
    now_unix_secs: u64,
) -> Result<Vec<ProviderQuotaAlertTarget>, GatewayError> {
    let providers = state
        .list_provider_catalog_providers(true)
        .await?
        .into_iter()
        .filter_map(|provider| {
            let config = provider_quota_alert_config(&provider)?;
            (config.enabled).then_some(ProviderQuotaAlertTarget { provider, config })
        })
        .collect::<Vec<_>>();
    let mut due = Vec::new();
    for target in providers {
        let runtime = read_quota_alert_runtime_state(state, &target.provider.id).await;
        let last_checked_at = runtime
            .as_ref()
            .map(|state| state.last_checked_at)
            .unwrap_or(0);
        if now_unix_secs.saturating_sub(last_checked_at)
            >= target
                .config
                .fetch_interval_seconds
                .max(PROVIDER_QUOTA_ALERT_MIN_FETCH_INTERVAL_SECS)
        {
            due.push(target);
        }
    }
    Ok(due)
}

async fn run_provider_quota_alert_for_provider(
    state: &AppState,
    target: ProviderQuotaAlertTarget,
    endpoints: Vec<StoredProviderCatalogEndpoint>,
) -> ProviderQuotaAlertStatus {
    let provider_id = target.provider.id.clone();
    let admin_state = AdminAppState::new(state);
    let payload = admin_provider_ops_local_action_response(
        &admin_state,
        &provider_id,
        Some(&target.provider),
        &endpoints,
        "query_balance",
        None,
    )
    .await;
    store_admin_provider_ops_balance_cache(&admin_state, &provider_id, &payload).await;

    let now_unix_secs = now_unix_secs();
    let payload_status_success = payload.get("status").and_then(Value::as_str) == Some("success");
    let Some(total_available) = extract_total_available(&payload) else {
        warn!(
            provider_id = %provider_id,
            payload = %payload,
            "provider quota alert skipped because balance payload has no total_available"
        );
        write_checked_runtime_state_without_balance(state, &provider_id, now_unix_secs).await;
        return if payload_status_success {
            ProviderQuotaAlertStatus::Skipped
        } else {
            ProviderQuotaAlertStatus::Failed
        };
    };

    let previous = read_quota_alert_runtime_state(state, &provider_id).await;
    let should_notify = provider_quota_alert_should_notify(
        now_unix_secs,
        total_available,
        target.config.threshold_amount,
        previous.as_ref(),
    );
    let mut next = ProviderQuotaAlertRuntimeState {
        last_checked_at: now_unix_secs,
        last_available: Some(total_available),
        below_threshold: total_available <= target.config.threshold_amount,
        last_notified_at: previous.and_then(|state| state.last_notified_at),
    };

    if should_notify {
        let notification = build_provider_quota_alert_notification(
            &target.provider,
            total_available,
            target.config.threshold_amount,
        );
        let variables = provider_quota_alert_notification_variables(
            &target.provider,
            total_available,
            target.config.threshold_amount,
        );
        let report = send_important_notification_for_item(
            state,
            PROVIDER_QUOTA_ALERT_ITEM_KEY,
            notification,
            &variables,
        )
        .await;
        let delivered = match &report {
            Ok(report) if report.success => true,
            Ok(report) => {
                warn!(
                    provider_id = %provider_id,
                    report = ?report,
                    "provider quota alert notification did not reach any channel"
                );
                false
            }
            Err(err) => {
                warn!(
                    provider_id = %provider_id,
                    error = ?err,
                    "provider quota alert notification failed"
                );
                false
            }
        };
        if delivered {
            next.last_notified_at = Some(now_unix_secs);
            write_quota_alert_runtime_state(state, &provider_id, &next).await;
            return ProviderQuotaAlertStatus::Alerted;
        }
        write_quota_alert_runtime_state(state, &provider_id, &next).await;
        return ProviderQuotaAlertStatus::Failed;
    }

    write_quota_alert_runtime_state(state, &provider_id, &next).await;
    ProviderQuotaAlertStatus::Checked
}

fn provider_quota_alert_config(
    provider: &StoredProviderCatalogProvider,
) -> Option<ProviderQuotaAlertConfig> {
    let quota_alert = provider
        .config
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|config| config.get("provider_ops"))
        .and_then(Value::as_object)
        .and_then(|provider_ops| provider_ops.get("quota_alert"))
        .and_then(Value::as_object)?;
    let enabled = quota_alert
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let threshold_amount = quota_alert
        .get("threshold_amount")
        .and_then(value_as_f64)
        .filter(|value| value.is_finite() && *value >= 0.0)
        .unwrap_or(0.0);
    let fetch_interval_seconds = quota_alert
        .get("fetch_interval_seconds")
        .and_then(Value::as_u64)
        .unwrap_or(PROVIDER_QUOTA_ALERT_DEFAULT_FETCH_INTERVAL_SECS)
        .max(PROVIDER_QUOTA_ALERT_MIN_FETCH_INTERVAL_SECS);
    Some(ProviderQuotaAlertConfig {
        enabled,
        threshold_amount,
        fetch_interval_seconds,
    })
}

fn provider_quota_alert_should_notify(
    now_unix_secs: u64,
    total_available: f64,
    threshold_amount: f64,
    previous: Option<&ProviderQuotaAlertRuntimeState>,
) -> bool {
    if total_available > threshold_amount {
        return false;
    }
    let Some(previous) = previous else {
        return true;
    };
    if !previous.below_threshold {
        return true;
    }
    previous
        .last_notified_at
        .map(|last| now_unix_secs.saturating_sub(last) >= PROVIDER_QUOTA_ALERT_REPEAT_COOLDOWN_SECS)
        .unwrap_or(true)
}

fn extract_total_available(payload: &Value) -> Option<f64> {
    if payload.get("status").and_then(Value::as_str) != Some("success") {
        return None;
    }
    payload
        .get("data")
        .and_then(|data| data.get("total_available"))
        .and_then(value_as_f64)
        .filter(|value| value.is_finite())
}

fn value_as_f64(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| {
        value
            .as_str()
            .and_then(|raw| raw.trim().parse::<f64>().ok())
    })
}

fn build_provider_quota_alert_notification(
    provider: &StoredProviderCatalogProvider,
    total_available: f64,
    threshold_amount: f64,
) -> ImportantNotification {
    let title = format!("提供商额度提醒：{}", provider.name);
    let body = format!(
        "提供商 `{}` 当前剩余额度为 `{:.4}`，已低于或等于提醒阈值 `{:.4}`。\n\nProvider ID: `{}`",
        provider.name, total_available, threshold_amount, provider.id
    );
    let text_body = format!(
        "提供商 {} 当前剩余额度为 {:.4}，已低于或等于提醒阈值 {:.4}。\n\nProvider ID: {}",
        provider.name, total_available, threshold_amount, provider.id
    );
    ImportantNotification {
        title,
        markdown_body: body,
        text_body,
    }
}

fn provider_quota_alert_notification_variables(
    provider: &StoredProviderCatalogProvider,
    total_available: f64,
    threshold_amount: f64,
) -> Vec<(&'static str, String)> {
    vec![
        ("provider_name", provider.name.clone()),
        ("provider_id", provider.id.clone()),
        ("total_available", format!("{total_available:.4}")),
        ("threshold_amount", format!("{threshold_amount:.4}")),
    ]
}

async fn read_quota_alert_runtime_state(
    state: &AppState,
    provider_id: &str,
) -> Option<ProviderQuotaAlertRuntimeState> {
    let key = provider_quota_alert_state_key(provider_id);
    state
        .runtime_kv_get(&key)
        .await
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str::<ProviderQuotaAlertRuntimeState>(&raw).ok())
}

async fn write_checked_runtime_state_without_balance(
    state: &AppState,
    provider_id: &str,
    now_unix_secs: u64,
) {
    let mut next = read_quota_alert_runtime_state(state, provider_id)
        .await
        .unwrap_or_default();
    next.last_checked_at = now_unix_secs;
    write_quota_alert_runtime_state(state, provider_id, &next).await;
}

async fn write_quota_alert_runtime_state(
    state: &AppState,
    provider_id: &str,
    runtime_state: &ProviderQuotaAlertRuntimeState,
) {
    let Ok(serialized) = serde_json::to_string(runtime_state) else {
        return;
    };
    if let Err(err) = state
        .runtime_state()
        .kv_set(
            &provider_quota_alert_state_key(provider_id),
            serialized,
            None,
        )
        .await
    {
        warn!(
            error = %err,
            provider_id,
            "failed to write provider quota alert runtime state"
        );
    }
}

fn provider_quota_alert_state_key(provider_id: &str) -> String {
    format!("{PROVIDER_QUOTA_ALERT_STATE_PREFIX}{provider_id}")
}

fn now_unix_secs() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

#[cfg(test)]
mod tests {
    use super::{
        provider_quota_alert_should_notify, ProviderQuotaAlertRuntimeState,
        PROVIDER_QUOTA_ALERT_REPEAT_COOLDOWN_SECS,
    };

    #[test]
    fn quota_alert_notifies_on_first_drop_and_after_cooldown() {
        assert!(provider_quota_alert_should_notify(100, 3.0, 5.0, None));
        assert!(provider_quota_alert_should_notify(
            100,
            3.0,
            5.0,
            Some(&ProviderQuotaAlertRuntimeState {
                below_threshold: false,
                ..ProviderQuotaAlertRuntimeState::default()
            })
        ));
        assert!(!provider_quota_alert_should_notify(
            100,
            3.0,
            5.0,
            Some(&ProviderQuotaAlertRuntimeState {
                below_threshold: true,
                last_notified_at: Some(90),
                ..ProviderQuotaAlertRuntimeState::default()
            })
        ));
        assert!(provider_quota_alert_should_notify(
            100 + PROVIDER_QUOTA_ALERT_REPEAT_COOLDOWN_SECS,
            3.0,
            5.0,
            Some(&ProviderQuotaAlertRuntimeState {
                below_threshold: true,
                last_notified_at: Some(100),
                ..ProviderQuotaAlertRuntimeState::default()
            })
        ));
    }

    #[test]
    fn quota_alert_does_not_notify_above_threshold() {
        assert!(!provider_quota_alert_should_notify(100, 6.0, 5.0, None));
    }
}
