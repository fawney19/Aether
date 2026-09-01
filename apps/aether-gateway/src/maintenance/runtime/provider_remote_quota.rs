use std::collections::HashMap;
use std::time::Duration;

use aether_admin::system::{
    ENABLE_PROVIDER_REMOTE_QUOTA_SYNC_CONFIG_KEY, PROVIDER_REMOTE_QUOTA_SYNC_INTERVAL_CONFIG_KEY,
    PROVIDER_REMOTE_QUOTA_SYNC_INTERVAL_DEFAULT_SECONDS,
    PROVIDER_REMOTE_QUOTA_SYNC_INTERVAL_MAX_SECONDS,
    PROVIDER_REMOTE_QUOTA_SYNC_INTERVAL_MIN_SECONDS,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogProvider,
};
use aether_data_contracts::DataLayerError;
use futures_util::stream::{self, StreamExt};
use serde_json::Value;
use tracing::warn;

use crate::admin_api::{
    admin_provider_ops_local_action_response, admin_provider_ops_remote_quota_worker_eligible,
    store_admin_provider_ops_balance_cache, AdminAppState,
};
use crate::data::GatewayDataState;
use crate::{AppState, GatewayError};

use super::{system_config_bool, system_config_u64, PROVIDER_REMOTE_QUOTA_SYNC_CONCURRENCY};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProviderRemoteQuotaSyncRunSummary {
    pub(crate) attempted: usize,
    pub(crate) applied: usize,
    pub(crate) skipped: usize,
    pub(crate) failed: usize,
}

pub(crate) async fn provider_remote_quota_sync_interval(
    data: &GatewayDataState,
) -> Result<Duration, DataLayerError> {
    let seconds = system_config_u64(
        data,
        PROVIDER_REMOTE_QUOTA_SYNC_INTERVAL_CONFIG_KEY,
        PROVIDER_REMOTE_QUOTA_SYNC_INTERVAL_DEFAULT_SECONDS,
    )
    .await?
    .clamp(
        PROVIDER_REMOTE_QUOTA_SYNC_INTERVAL_MIN_SECONDS,
        PROVIDER_REMOTE_QUOTA_SYNC_INTERVAL_MAX_SECONDS,
    );
    Ok(Duration::from_secs(seconds))
}

pub(crate) async fn perform_provider_remote_quota_sync_once(
    state: &AppState,
) -> Result<ProviderRemoteQuotaSyncRunSummary, GatewayError> {
    if !state.has_provider_catalog_data_reader() || !state.has_provider_quota_data_writer() {
        return Ok(empty_summary());
    }
    if !system_config_bool(
        &state.data,
        ENABLE_PROVIDER_REMOTE_QUOTA_SYNC_CONFIG_KEY,
        true,
    )
    .await
    .map_err(|error| GatewayError::Internal(error.to_string()))?
    {
        return Ok(empty_summary());
    }

    let providers = state
        .list_provider_catalog_providers(true)
        .await?
        .into_iter()
        .filter(admin_provider_ops_remote_quota_worker_eligible)
        .collect::<Vec<_>>();
    if providers.is_empty() {
        return Ok(empty_summary());
    }

    let provider_ids = providers
        .iter()
        .map(|provider| provider.id.clone())
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

    let mut results = stream::iter(providers.into_iter().map(|provider| {
        let state = state.clone();
        let provider_id = provider.id.clone();
        let endpoints = endpoints_by_provider
            .remove(&provider_id)
            .unwrap_or_default();
        async move { sync_provider_remote_quota(&state, provider, endpoints).await }
    }))
    .buffer_unordered(PROVIDER_REMOTE_QUOTA_SYNC_CONCURRENCY);

    let mut summary = ProviderRemoteQuotaSyncRunSummary {
        attempted: provider_ids.len(),
        applied: 0,
        skipped: 0,
        failed: 0,
    };
    while let Some((provider_id, outcome, message)) = results.next().await {
        match outcome {
            ProviderRemoteQuotaWorkerOutcome::Applied => summary.applied += 1,
            ProviderRemoteQuotaWorkerOutcome::Skipped => summary.skipped += 1,
            ProviderRemoteQuotaWorkerOutcome::Failed => {
                summary.failed += 1;
                warn!(
                    provider_id = %provider_id,
                    message = %message,
                    "provider remote quota sync failed; local quota was kept"
                );
            }
        }
    }
    Ok(summary)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderRemoteQuotaWorkerOutcome {
    Applied,
    Skipped,
    Failed,
}

async fn sync_provider_remote_quota(
    state: &AppState,
    provider: StoredProviderCatalogProvider,
    endpoints: Vec<StoredProviderCatalogEndpoint>,
) -> (String, ProviderRemoteQuotaWorkerOutcome, String) {
    let provider_id = provider.id.clone();
    let admin_state = AdminAppState::new(state);
    let payload = admin_provider_ops_local_action_response(
        &admin_state,
        &provider_id,
        Some(&provider),
        &endpoints,
        "query_balance",
        None,
    )
    .await;
    store_admin_provider_ops_balance_cache(&admin_state, &provider_id, &payload).await;
    let sync_status = payload
        .pointer("/data/extra/remote_quota_sync/status")
        .and_then(Value::as_str);
    let outcome = if payload.get("status").and_then(Value::as_str) != Some("success") {
        ProviderRemoteQuotaWorkerOutcome::Failed
    } else {
        match sync_status {
            Some("applied") => ProviderRemoteQuotaWorkerOutcome::Applied,
            Some("stale_window" | "concurrent_update" | "skipped_kill_switch") => {
                ProviderRemoteQuotaWorkerOutcome::Skipped
            }
            _ => ProviderRemoteQuotaWorkerOutcome::Failed,
        }
    };
    let message = if outcome == ProviderRemoteQuotaWorkerOutcome::Applied {
        "applied".to_string()
    } else {
        payload
            .pointer("/error/message")
            .and_then(Value::as_str)
            .or_else(|| {
                payload
                    .pointer("/data/extra/remote_quota_sync/message")
                    .and_then(Value::as_str)
            })
            .or_else(|| payload.get("message").and_then(Value::as_str))
            .unwrap_or("remote quota response did not report an applied snapshot")
            .to_string()
    };
    (provider_id, outcome, message)
}

const fn empty_summary() -> ProviderRemoteQuotaSyncRunSummary {
    ProviderRemoteQuotaSyncRunSummary {
        attempted: 0,
        applied: 0,
        skipped: 0,
        failed: 0,
    }
}

#[cfg(test)]
mod tests {
    use crate::admin_api::admin_provider_ops_remote_quota_worker_eligible;
    use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider;
    use serde_json::json;

    fn provider(config: serde_json::Value) -> StoredProviderCatalogProvider {
        let mut provider = StoredProviderCatalogProvider::new(
            "provider-1".to_string(),
            "Provider".to_string(),
            None,
            "openai".to_string(),
        )
        .expect("provider should build");
        provider.config = Some(config);
        provider
    }

    #[test]
    fn selects_only_enabled_sub2api_remote_quota() {
        assert!(admin_provider_ops_remote_quota_worker_eligible(&provider(
            json!({
                "provider_ops": {
                    "architecture_id": "sub2api",
                    "remote_quota": {
                        "enabled": true,
                        "group_id": "42"
                    }
                }
            })
        )));
        assert!(!admin_provider_ops_remote_quota_worker_eligible(&provider(
            json!({
                "provider_ops": {
                    "architecture_id": "sub2api",
                    "remote_quota": {"enabled": false, "group_id": "42"}
                }
            })
        )));
        assert!(!admin_provider_ops_remote_quota_worker_eligible(&provider(
            json!({
                "provider_ops": {
                    "architecture_id": "sub2api",
                    "remote_quota": {"enabled": true}
                }
            })
        )));
        assert!(!admin_provider_ops_remote_quota_worker_eligible(&provider(
            json!({
                "provider_ops": {
                    "architecture_id": "generic_api",
                    "remote_quota": {"enabled": true, "group_id": "42"}
                }
            })
        )));
    }
}
