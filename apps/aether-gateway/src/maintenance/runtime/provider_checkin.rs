use std::collections::HashMap;

use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogProvider,
};
use futures_util::stream::{self, StreamExt};
use tracing::{debug, warn};

use crate::handlers::admin::provider::ops::providers::actions::admin_provider_ops_local_action_response;
use crate::{AppState, GatewayError};

use super::{system_config_bool, PROVIDER_CHECKIN_CONCURRENCY};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProviderCheckinRunSummary {
    pub(crate) attempted: usize,
    pub(crate) succeeded: usize,
    pub(crate) failed: usize,
    pub(crate) skipped: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderCheckinStatus {
    Succeeded,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProviderCheckinOutcome {
    provider_id: String,
    status: ProviderCheckinStatus,
    message: String,
}

pub(crate) async fn perform_provider_checkin_once(
    state: &AppState,
) -> Result<ProviderCheckinRunSummary, GatewayError> {
    if !system_config_bool(&state.data, "enable_provider_checkin", true)
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?
    {
        return Ok(ProviderCheckinRunSummary {
            attempted: 0,
            succeeded: 0,
            failed: 0,
            skipped: 0,
        });
    }

    let providers = state
        .list_provider_catalog_providers(true)
        .await?
        .into_iter()
        .filter(provider_has_ops_config)
        .collect::<Vec<_>>();
    if providers.is_empty() {
        return Ok(ProviderCheckinRunSummary {
            attempted: 0,
            succeeded: 0,
            failed: 0,
            skipped: 0,
        });
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
        async move { run_provider_checkin_for_provider(&state, provider, endpoints).await }
    }))
    .buffer_unordered(PROVIDER_CHECKIN_CONCURRENCY);

    let mut summary = ProviderCheckinRunSummary {
        attempted: provider_ids.len(),
        succeeded: 0,
        failed: 0,
        skipped: 0,
    };
    while let Some(outcome) = results.next().await {
        match outcome.status {
            ProviderCheckinStatus::Succeeded => summary.succeeded += 1,
            ProviderCheckinStatus::Failed => {
                summary.failed += 1;
                warn!(
                    provider_id = %outcome.provider_id,
                    message = %outcome.message,
                    "gateway provider checkin failed"
                );
            }
            ProviderCheckinStatus::Skipped => {
                summary.skipped += 1;
                debug!(
                    provider_id = %outcome.provider_id,
                    message = %outcome.message,
                    "gateway provider checkin skipped"
                );
            }
        }
    }

    Ok(summary)
}

fn provider_has_ops_config(provider: &StoredProviderCatalogProvider) -> bool {
    provider
        .config
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|config| config.get("provider_ops"))
        .and_then(serde_json::Value::as_object)
        .is_some_and(|config| !config.is_empty())
}

async fn run_provider_checkin_for_provider(
    state: &AppState,
    provider: StoredProviderCatalogProvider,
    endpoints: Vec<StoredProviderCatalogEndpoint>,
) -> ProviderCheckinOutcome {
    let provider_id = provider.id.clone();
    let payload = admin_provider_ops_local_action_response(
        state,
        &provider_id,
        Some(&provider),
        &endpoints,
        "query_balance",
        None,
    )
    .await;
    provider_checkin_outcome_from_payload(&provider_id, &payload)
}

fn provider_checkin_outcome_from_payload(
    provider_id: &str,
    payload: &serde_json::Value,
) -> ProviderCheckinOutcome {
    let default_message = || "未执行签到".to_string();
    let payload_status = payload
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let payload_message = payload
        .get("message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();

    let (status, message) = if payload_status == "success" {
        let extra = payload
            .get("data")
            .and_then(|value| value.get("extra"))
            .and_then(serde_json::Value::as_object);
        let checkin_message = extra
            .and_then(|extra| extra.get("checkin_message"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(default_message);
        match extra
            .and_then(|extra| extra.get("checkin_success"))
            .and_then(serde_json::Value::as_bool)
        {
            Some(true) => (ProviderCheckinStatus::Succeeded, checkin_message),
            Some(false) => (ProviderCheckinStatus::Failed, checkin_message),
            None => (ProviderCheckinStatus::Skipped, checkin_message),
        }
    } else if payload_status == "not_supported" {
        (
            ProviderCheckinStatus::Skipped,
            if payload_message.is_empty() {
                default_message()
            } else {
                payload_message
            },
        )
    } else {
        (
            ProviderCheckinStatus::Failed,
            if payload_message.is_empty() {
                "签到失败".to_string()
            } else {
                payload_message
            },
        )
    };

    ProviderCheckinOutcome {
        provider_id: provider_id.to_string(),
        status,
        message,
    }
}
