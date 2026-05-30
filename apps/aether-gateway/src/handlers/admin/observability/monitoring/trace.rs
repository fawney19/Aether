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
    candidates::{
        DecisionTrace, DecisionTraceCandidate, RequestCandidateFinalStatus, RequestCandidateStatus,
        StoredRequestCandidate,
    },
    provider_catalog::StoredProviderCatalogKey,
    usage::StoredRequestUsageAudit,
};
use axum::{
    body::Body,
    response::{IntoResponse, Response},
};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use tracing::debug;

struct ResolvedAdminMonitoringTrace {
    trace: DecisionTrace,
    usage: Option<StoredRequestUsageAudit>,
}

pub(super) async fn build_admin_monitoring_trace_request_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let admin_state = state;
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

    let Some(resolved) =
        resolve_admin_monitoring_trace(admin_state, &request_id, attempted_only).await?
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
    let key_accounts =
        build_admin_monitoring_key_account_display_map(admin_state, &resolved.trace).await?;

    Ok(
        build_admin_monitoring_trace_request_payload_response_with_key_accounts(
            &resolved.trace,
            resolved.usage.as_ref(),
            &key_accounts,
        ),
    )
}

async fn resolve_admin_monitoring_trace(
    state: &AdminAppState<'_>,
    request_id: &str,
    attempted_only: bool,
) -> Result<Option<ResolvedAdminMonitoringTrace>, GatewayError> {
    let app = state.as_ref();
    if let Some(trace) = app
        .data
        .read_decision_trace(request_id, attempted_only)
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?
    {
        let usage = app
            .data
            .read_request_usage_audit(request_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        return Ok(Some(ResolvedAdminMonitoringTrace { trace, usage }));
    }

    let mut usage_candidates = Vec::new();
    if let Some(usage) = app
        .data
        .read_request_usage_audit(request_id)
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?
    {
        usage_candidates.push(usage);
    }
    if let Some(usage) = state.find_request_usage_by_id(request_id).await? {
        if !usage_candidates.iter().any(|item| item.id == usage.id) {
            usage_candidates.push(usage);
        }
    }

    let mut usage_snapshot_fallback = None;
    for usage in usage_candidates {
        for trace_request_id in admin_monitoring_usage_trace_request_ids(&usage) {
            if trace_request_id == request_id {
                continue;
            }
            if let Some(trace) = app
                .data
                .read_decision_trace(&trace_request_id, attempted_only)
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?
            {
                return Ok(Some(ResolvedAdminMonitoringTrace {
                    trace,
                    usage: Some(usage),
                }));
            }
        }
        if usage_snapshot_fallback.is_none() {
            if let Some(trace) = build_admin_monitoring_usage_routing_snapshot_trace(&usage) {
                usage_snapshot_fallback = Some(ResolvedAdminMonitoringTrace {
                    trace,
                    usage: Some(usage.clone()),
                });
            }
        }
    }

    Ok(usage_snapshot_fallback)
}

fn build_admin_monitoring_usage_routing_snapshot_trace(
    usage: &StoredRequestUsageAudit,
) -> Option<DecisionTrace> {
    if !admin_monitoring_usage_has_routing_snapshot_trace_data(usage) {
        return None;
    }

    let status = admin_monitoring_usage_candidate_status(usage);
    let final_status = match status {
        RequestCandidateStatus::Success => RequestCandidateFinalStatus::Success,
        RequestCandidateStatus::Cancelled => RequestCandidateFinalStatus::Cancelled,
        RequestCandidateStatus::Streaming => RequestCandidateFinalStatus::Streaming,
        RequestCandidateStatus::Pending => RequestCandidateFinalStatus::Pending,
        RequestCandidateStatus::Available
        | RequestCandidateStatus::Unused
        | RequestCandidateStatus::Failed
        | RequestCandidateStatus::Skipped => RequestCandidateFinalStatus::Failed,
    };
    let latency_ms = usage.response_time_ms.unwrap_or_default();
    let candidate = StoredRequestCandidate {
        id: usage
            .routing_candidate_id()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("usage-routing-snapshot:{}", usage.id)),
        request_id: admin_monitoring_usage_primary_trace_request_id(usage),
        user_id: usage.user_id.clone(),
        api_key_id: usage.api_key_id.clone(),
        username: usage.username.clone(),
        api_key_name: usage.api_key_name.clone(),
        candidate_index: usage
            .routing_candidate_index()
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(0),
        retry_index: 0,
        provider_id: usage.provider_id.clone(),
        endpoint_id: usage.provider_endpoint_id.clone(),
        key_id: usage.provider_api_key_id.clone(),
        status,
        skip_reason: None,
        is_cached: false,
        status_code: usage.status_code,
        error_type: usage
            .routing_local_execution_runtime_miss_reason()
            .or(usage.error_category.as_deref())
            .map(ToOwned::to_owned),
        error_message: usage.error_message.clone(),
        latency_ms: usage.response_time_ms,
        concurrent_requests: None,
        extra_data: build_admin_monitoring_usage_routing_snapshot_extra_data(usage),
        required_capabilities: None,
        created_at_unix_ms: usage.created_at_unix_ms,
        started_at_unix_ms: Some(usage.created_at_unix_ms),
        finished_at_unix_ms: admin_monitoring_usage_finished_at_unix_ms(usage),
    };

    Some(DecisionTrace {
        request_id: candidate.request_id.clone(),
        total_candidates: 1,
        final_status,
        total_latency_ms: latency_ms,
        candidates: vec![DecisionTraceCandidate {
            candidate,
            provider_name: non_empty_string(usage.provider_name.as_str()),
            provider_website: None,
            provider_type: None,
            provider_priority: None,
            provider_keep_priority_on_conversion: None,
            provider_enable_format_conversion: None,
            endpoint_api_format: usage
                .endpoint_api_format
                .clone()
                .or_else(|| usage.api_format.clone()),
            endpoint_api_family: usage
                .provider_api_family
                .clone()
                .or_else(|| usage.api_family.clone()),
            endpoint_kind: usage
                .provider_endpoint_kind
                .clone()
                .or_else(|| usage.endpoint_kind.clone()),
            endpoint_format_acceptance_config: None,
            provider_key_name: usage.routing_key_name().map(ToOwned::to_owned),
            provider_key_auth_type: None,
            provider_key_api_formats: None,
            provider_key_internal_priority: None,
            provider_key_global_priority_by_format: None,
            provider_key_capabilities: None,
            provider_key_is_active: None,
        }],
    })
}

fn admin_monitoring_usage_has_routing_snapshot_trace_data(usage: &StoredRequestUsageAudit) -> bool {
    usage.routing_candidate_id().is_some()
        || usage.routing_candidate_index().is_some()
        || usage.routing_execution_path().is_some()
        || usage
            .routing_local_execution_runtime_miss_reason()
            .is_some()
}

fn admin_monitoring_usage_candidate_status(
    usage: &StoredRequestUsageAudit,
) -> RequestCandidateStatus {
    if usage.status.trim().eq_ignore_ascii_case("cancelled")
        || usage.status.trim().eq_ignore_ascii_case("canceled")
    {
        return RequestCandidateStatus::Cancelled;
    }

    match usage.status_code {
        Some(status_code) if (200..300).contains(&status_code) => RequestCandidateStatus::Success,
        Some(_) => RequestCandidateStatus::Failed,
        None if usage.status.trim().eq_ignore_ascii_case("completed")
            || usage.status.trim().eq_ignore_ascii_case("success") =>
        {
            RequestCandidateStatus::Success
        }
        None => RequestCandidateStatus::Failed,
    }
}

fn admin_monitoring_usage_primary_trace_request_id(usage: &StoredRequestUsageAudit) -> String {
    if let Some(trace_id) = usage.trace_id() {
        return trace_id.to_string();
    }
    if let Some(trace_id) = usage_trace_id_from_headers(usage.request_headers.as_ref()) {
        return trace_id;
    }
    if let Some(trace_id) = usage_trace_id_from_headers(usage.provider_request_headers.as_ref()) {
        return trace_id;
    }
    usage.request_id.clone()
}

fn admin_monitoring_usage_finished_at_unix_ms(usage: &StoredRequestUsageAudit) -> Option<u64> {
    usage
        .finalized_at_unix_secs
        .map(|value| value.saturating_mul(1_000))
        .or_else(|| {
            usage
                .response_time_ms
                .map(|latency_ms| usage.created_at_unix_ms.saturating_add(latency_ms))
        })
}

fn build_admin_monitoring_usage_routing_snapshot_extra_data(
    usage: &StoredRequestUsageAudit,
) -> Option<Value> {
    let mut object = Map::new();
    object.insert("source".to_string(), json!("usage_routing_snapshot"));
    insert_optional_string(&mut object, "planner_kind", usage.routing_planner_kind());
    insert_optional_string(&mut object, "route_family", usage.routing_route_family());
    insert_optional_string(&mut object, "route_kind", usage.routing_route_kind());
    insert_optional_string(
        &mut object,
        "execution_path",
        usage.routing_execution_path(),
    );
    insert_optional_string(
        &mut object,
        "local_execution_runtime_miss_reason",
        usage.routing_local_execution_runtime_miss_reason(),
    );
    insert_optional_string(&mut object, "key_name", usage.routing_key_name());
    insert_optional_string(&mut object, "model", Some(usage.model.as_str()));
    insert_optional_string(&mut object, "target_model", usage.target_model.as_deref());
    insert_optional_string(
        &mut object,
        "client_api_format",
        usage.api_format.as_deref(),
    );
    insert_optional_string(
        &mut object,
        "provider_api_format",
        usage.endpoint_api_format.as_deref(),
    );
    insert_optional_string(
        &mut object,
        "provider_api_family",
        usage.provider_api_family.as_deref(),
    );
    insert_optional_string(
        &mut object,
        "provider_endpoint_kind",
        usage.provider_endpoint_kind.as_deref(),
    );
    insert_optional_string(&mut object, "candidate_id", usage.routing_candidate_id());
    if let Some(candidate_index) = usage.routing_candidate_index() {
        object.insert("candidate_index".to_string(), json!(candidate_index));
    }
    Some(Value::Object(object))
}

fn admin_monitoring_usage_trace_request_ids(usage: &StoredRequestUsageAudit) -> Vec<String> {
    let mut ids = Vec::new();
    push_non_empty_unique(&mut ids, usage.request_id.as_str());
    if let Some(trace_id) = usage.trace_id() {
        push_non_empty_unique(&mut ids, trace_id);
    }
    if let Some(trace_id) = usage_trace_id_from_headers(usage.request_headers.as_ref()) {
        push_non_empty_unique(&mut ids, trace_id.as_str());
    }
    if let Some(trace_id) = usage_trace_id_from_headers(usage.provider_request_headers.as_ref()) {
        push_non_empty_unique(&mut ids, trace_id.as_str());
    }
    ids
}

fn usage_trace_id_from_headers(headers: Option<&Value>) -> Option<String> {
    let object = headers?.as_object()?;
    object.iter().find_map(|(key, value)| {
        key.eq_ignore_ascii_case(crate::constants::TRACE_ID_HEADER)
            .then(|| {
                value
                    .as_str()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
            })
            .flatten()
            .map(ToOwned::to_owned)
    })
}

fn push_non_empty_unique(values: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if value.is_empty() || values.iter().any(|existing| existing == value) {
        return;
    }
    values.push(value.to_string());
}

fn non_empty_string(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn insert_optional_string(object: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    let Some(value) = value.and_then(non_empty_string) else {
        return;
    };
    object.insert(key.to_string(), Value::String(value));
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
