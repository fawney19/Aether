mod metadata;
mod persistence;
mod probe;

use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use aether_admin::provider::quota as admin_provider_quota_pure;
use aether_contracts::ProxySnapshot;
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use serde_json::json;

use metadata::{
    key_write_failed_result, missing_key_result, transport_unavailable_result, unix_now,
};
use persistence::{persist_empty_probe_result, persist_success_result};

const GLM_CODING_PLAN_5H_WINDOW_SECONDS: u64 = 5 * 60 * 60;
const GLM_CODING_PLAN_WEEKLY_WINDOW_SECONDS: u64 = 7 * 24 * 60 * 60;

enum RefreshOneKeyResult {
    MissingKey,
    TransportUnavailable,
    Outcome(RefreshOutcome),
}

pub(super) struct RefreshOutcome {
    pub(super) metadata: serde_json::Map<String, serde_json::Value>,
    pub(super) parsed_probe_count: usize,
    pub(super) oauth_invalid_at_unix_secs: Option<u64>,
    pub(super) oauth_invalid_reason: Option<String>,
}

pub(crate) async fn refresh_glm_coding_plan_provider_quota_locally(
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
        match refresh_one_key(state, provider, endpoint, &key, proxy_override.as_ref()).await? {
            RefreshOneKeyResult::MissingKey => {
                failed_count += 1;
                results.push(missing_key_result(&key));
            }
            RefreshOneKeyResult::TransportUnavailable => {
                failed_count += 1;
                results.push(transport_unavailable_result(&key));
            }
            RefreshOneKeyResult::Outcome(outcome) => {
                if outcome.parsed_probe_count == 0 {
                    failed_count += 1;
                    results.push(
                        persist_empty_probe_result(state, &key, outcome)
                            .await?
                            .unwrap_or_else(|| key_write_failed_result(&key)),
                    );
                } else {
                    match persist_success_result(state, &key, outcome).await? {
                        Some(result) => {
                            success_count += 1;
                            results.push(result);
                        }
                        None => {
                            failed_count += 1;
                            results.push(key_write_failed_result(&key));
                        }
                    }
                }
            }
        }
    }

    Ok(Some(json!({
        "success": success_count,
        "failed": failed_count,
        "total": results.len(),
        "results": results,
        "message": format!("已处理 {} 个 Key", results.len()),
        "auto_removed": 0,
    })))
}

async fn refresh_one_key(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    endpoint: &StoredProviderCatalogEndpoint,
    key: &StoredProviderCatalogKey,
    proxy_override: Option<&ProxySnapshot>,
) -> Result<RefreshOneKeyResult, GatewayError> {
    let Some(transport) = state
        .read_provider_transport_snapshot(&provider.id, &endpoint.id, &key.id)
        .await?
    else {
        return Ok(RefreshOneKeyResult::TransportUnavailable);
    };
    let api_key = transport.key.decrypted_api_key.trim();
    if api_key.is_empty() || api_key == "__placeholder__" {
        return Ok(RefreshOneKeyResult::MissingKey);
    }

    let now_unix_secs = unix_now();
    let mut outcome = RefreshOutcome {
        metadata: serde_json::Map::from_iter([("updated_at".to_string(), json!(now_unix_secs))]),
        parsed_probe_count: 0,
        oauth_invalid_at_unix_secs: None,
        oauth_invalid_reason: None,
    };

    let quota_result = probe::execute_kind(
        state,
        &transport,
        endpoint,
        api_key,
        "quota_limit",
        None,
        proxy_override,
    )
    .await?;
    probe::handle_probe_result(
        "quota_limit",
        probe::GlmCodingPlanProbeParser::QuotaLimit,
        quota_result,
        now_unix_secs,
        &mut outcome,
    );

    if let Some(query) = probe::usage_query_for_cycle_window(
        outcome
            .metadata
            .get("token_5h_reset_at")
            .and_then(admin_provider_quota_pure::coerce_json_u64),
        GLM_CODING_PLAN_5H_WINDOW_SECONDS,
        now_unix_secs,
        true,
    ) {
        let result = probe::execute_kind(
            state,
            &transport,
            endpoint,
            api_key,
            "model_usage",
            Some(&query),
            proxy_override,
        )
        .await?;
        probe::handle_probe_result(
            "model_usage_5h",
            probe::GlmCodingPlanProbeParser::Usage {
                kind: "model",
                window: Some("5h"),
            },
            result,
            now_unix_secs,
            &mut outcome,
        );
    }

    if let Some(query) = probe::usage_query_for_cycle_window(
        outcome
            .metadata
            .get("token_weekly_reset_at")
            .and_then(admin_provider_quota_pure::coerce_json_u64),
        GLM_CODING_PLAN_WEEKLY_WINDOW_SECONDS,
        now_unix_secs,
        false,
    ) {
        let result = probe::execute_kind(
            state,
            &transport,
            endpoint,
            api_key,
            "model_usage",
            Some(&query),
            proxy_override,
        )
        .await?;
        probe::handle_probe_result(
            "model_usage_weekly",
            probe::GlmCodingPlanProbeParser::Usage {
                kind: "model",
                window: Some("weekly"),
            },
            result,
            now_unix_secs,
            &mut outcome,
        );
    }

    Ok(RefreshOneKeyResult::Outcome(outcome))
}
