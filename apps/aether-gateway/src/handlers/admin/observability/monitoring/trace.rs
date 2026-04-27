use super::route_filters::parse_admin_monitoring_limit;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::log_ids::short_request_id;
use crate::GatewayError;
use aether_admin::observability::monitoring::{
    admin_monitoring_bad_request_response, admin_monitoring_trace_not_found_response,
    admin_monitoring_trace_provider_id_from_path, admin_monitoring_trace_request_id_from_path,
    build_admin_monitoring_trace_provider_stats_payload_response,
    build_admin_monitoring_trace_request_payload_response_with_key_accounts,
    parse_admin_monitoring_attempted_only, AdminMonitoringKeyAccountDisplay,
};
use aether_data_contracts::repository::{
    candidates::{DecisionTrace, RequestCandidateStatus},
    provider_catalog::StoredProviderCatalogKey,
};
use axum::{
    body::Body,
    response::{IntoResponse, Response},
};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use tracing::debug;

pub(super) async fn build_admin_monitoring_trace_request_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let admin_state = state;
    let state = state.as_ref();
    let Some(request_id) =
        admin_monitoring_trace_request_id_from_path(&request_context.request_path)
    else {
        return Ok(admin_monitoring_bad_request_response("缺少 request_id"));
    };
    let attempted_only = match parse_admin_monitoring_attempted_only(
        request_context.request_query_string.as_deref(),
    ) {
        Ok(value) => value,
        Err(detail) => return Ok(admin_monitoring_bad_request_response(detail)),
    };

    let Some(trace) = state
        .data
        .read_decision_trace(&request_id, attempted_only)
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?
    else {
        debug!(
            event_name = "admin_monitoring_request_trace_not_found",
            log_type = "admin_monitoring",
            request_id = %short_request_id(request_id.as_str()),
            attempted_only,
            path = %request_context.request_path,
            "admin monitoring request trace not found"
        );
        return Ok(admin_monitoring_trace_not_found_response(
            &request_id,
            attempted_only,
        ));
    };
    let usage = state
        .data
        .read_request_usage_audit(&request_id)
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let key_accounts = build_admin_monitoring_key_account_display_map(admin_state, &trace).await?;

    Ok(
        build_admin_monitoring_trace_request_payload_response_with_key_accounts(
            &trace,
            usage.as_ref(),
            &key_accounts,
        ),
    )
}

async fn build_admin_monitoring_key_account_display_map(
    state: &AdminAppState<'_>,
    trace: &DecisionTrace,
) -> Result<BTreeMap<String, AdminMonitoringKeyAccountDisplay>, GatewayError> {
    let key_ids = trace
        .candidates
        .iter()
        .filter_map(|item| item.candidate.key_id.as_deref())
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if key_ids.is_empty() {
        return Ok(BTreeMap::new());
    }

    let keys = state.read_provider_catalog_keys_by_ids(&key_ids).await?;
    Ok(keys
        .into_iter()
        .filter_map(|key| {
            let display = resolve_admin_monitoring_key_account_display(state, &key)?;
            Some((key.id, display))
        })
        .collect())
}

fn resolve_admin_monitoring_key_account_display(
    state: &AdminAppState<'_>,
    key: &StoredProviderCatalogKey,
) -> Option<AdminMonitoringKeyAccountDisplay> {
    let auth_config = parse_admin_monitoring_key_auth_config(state, key);
    let label = auth_config
        .as_ref()
        .and_then(|config| {
            first_non_empty_json_string([
                config.get("email"),
                config.get("account_name"),
                config.get("accountName"),
                config.get("client_email"),
                config.get("account_id"),
                config.get("accountId"),
            ])
        })
        .or_else(|| {
            key.upstream_metadata.as_ref().and_then(|metadata| {
                first_non_empty_json_string([
                    metadata.get("email"),
                    metadata.get("account_name"),
                    metadata.get("accountName"),
                    metadata.get("account_id"),
                    metadata.get("accountId"),
                ])
            })
        });
    let oauth_plan_type = auth_config.as_ref().and_then(|config| {
        first_non_empty_json_string([config.get("plan_type"), config.get("planType")])
    });

    if label.is_none() && oauth_plan_type.is_none() {
        return None;
    }

    Some(AdminMonitoringKeyAccountDisplay {
        label,
        oauth_plan_type,
    })
}

fn parse_admin_monitoring_key_auth_config(
    state: &AdminAppState<'_>,
    key: &StoredProviderCatalogKey,
) -> Option<Map<String, Value>> {
    let ciphertext = key.encrypted_auth_config.as_deref()?;
    let plaintext = state.decrypt_catalog_secret_with_fallbacks(ciphertext)?;
    serde_json::from_str::<Value>(&plaintext)
        .ok()?
        .as_object()
        .cloned()
}

fn first_non_empty_json_string<'a>(
    values: impl IntoIterator<Item = Option<&'a Value>>,
) -> Option<String> {
    values.into_iter().find_map(|value| {
        let text = value?.as_str()?.trim();
        (!text.is_empty()).then(|| text.to_string())
    })
}

pub(super) async fn build_admin_monitoring_trace_provider_stats_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let state = state.as_ref();
    let Some(provider_id) =
        admin_monitoring_trace_provider_id_from_path(&request_context.request_path)
    else {
        return Ok(admin_monitoring_bad_request_response("缺少 provider_id"));
    };
    let limit = match parse_admin_monitoring_limit(request_context.request_query_string.as_deref())
    {
        Ok(value) => value,
        Err(detail) => return Ok(admin_monitoring_bad_request_response(detail)),
    };

    let candidates = state
        .read_request_candidates_by_provider_id(&provider_id, limit)
        .await?;
    let total_attempts = candidates.len();
    let success_count = candidates
        .iter()
        .filter(|item| item.status == RequestCandidateStatus::Success)
        .count();
    let failed_count = candidates
        .iter()
        .filter(|item| item.status == RequestCandidateStatus::Failed)
        .count();
    let cancelled_count = candidates
        .iter()
        .filter(|item| item.status == RequestCandidateStatus::Cancelled)
        .count();
    let skipped_count = candidates
        .iter()
        .filter(|item| item.status == RequestCandidateStatus::Skipped)
        .count();
    let pending_count = candidates
        .iter()
        .filter(|item| item.status == RequestCandidateStatus::Pending)
        .count();
    let available_count = candidates
        .iter()
        .filter(|item| item.status == RequestCandidateStatus::Available)
        .count();
    let unused_count = candidates
        .iter()
        .filter(|item| item.status == RequestCandidateStatus::Unused)
        .count();
    let completed_count = success_count + failed_count;
    let failure_rate = if completed_count == 0 {
        0.0
    } else {
        ((failed_count as f64 / completed_count as f64) * 10000.0).round() / 100.0
    };
    let latency_values = candidates
        .iter()
        .filter_map(|item| item.latency_ms.map(|value| value as f64))
        .collect::<Vec<_>>();
    let avg_latency_ms = if latency_values.is_empty() {
        0.0
    } else {
        let total = latency_values.iter().sum::<f64>();
        ((total / latency_values.len() as f64) * 100.0).round() / 100.0
    };

    Ok(
        build_admin_monitoring_trace_provider_stats_payload_response(
            provider_id,
            total_attempts,
            success_count,
            failed_count,
            cancelled_count,
            skipped_count,
            pending_count,
            available_count,
            unused_count,
            failure_rate,
            avg_latency_ms,
        ),
    )
}
