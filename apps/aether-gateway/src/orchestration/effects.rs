use std::collections::BTreeMap;

use aether_admin::provider::quota as admin_provider_quota_pure;
use aether_contracts::{ExecutionPlan, ExecutionTelemetry};
use aether_data_contracts::repository::pool_scores::{
    PoolMemberHardState, PoolMemberIdentity, PoolMemberScheduleFeedback,
};
use aether_scheduler_core::{
    build_scheduler_affinity_cache_key_for_api_key_id_with_client_session,
    count_recent_rpm_requests_for_provider_key, ClientSessionAffinity, SchedulerAffinityTarget,
};
use aether_usage_runtime::{
    build_stream_terminal_usage_outcome, build_sync_terminal_usage_outcome,
    GatewayStreamReportRequest, GatewaySyncReportRequest, TerminalUsageOutcome,
};
use serde_json::Value;
use tracing::warn;

use super::{
    local_failover_error_message, project_local_adaptive_rate_limit,
    project_local_adaptive_success, project_local_failure_health, project_local_key_circuit_closed,
    project_local_key_circuit_failure, project_local_success_health, LocalFailoverClassification,
};
use crate::ai_serving::extract_pool_sticky_session_token;
use crate::client_session_affinity::{
    client_session_affinity_from_report_context_value, CLIENT_SESSION_AFFINITY_REPORT_CONTEXT_FIELD,
};
use crate::clock::current_unix_secs;
use crate::handlers::shared::provider_pool::admin_provider_pool_config_from_config_value;
use crate::handlers::shared::provider_pool::{
    admin_provider_pool_key_terminal_error_reason, record_admin_provider_pool_error,
    record_admin_provider_pool_stream_timeout, record_admin_provider_pool_success,
    release_admin_provider_pool_key_lease, AdminProviderPoolConfig,
};
use crate::orchestration::local_execution_candidate_metadata_from_report_context;
use crate::scheduler::affinity::SCHEDULER_AFFINITY_TTL;
use crate::scheduler::config::{read_scheduler_ordering_config, SchedulerSchedulingMode};
use crate::AppState;

#[derive(Debug, Clone, Copy)]
pub(crate) struct LocalExecutionEffectContext<'a> {
    pub(crate) plan: &'a ExecutionPlan,
    pub(crate) report_context: Option<&'a Value>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct LocalPoolErrorEffect<'a> {
    pub(crate) status_code: u16,
    pub(crate) classification: LocalFailoverClassification,
    pub(crate) headers: &'a BTreeMap<String, String>,
    pub(crate) error_body: Option<&'a str>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct LocalAttemptFailureEffect {
    pub(crate) status_code: u16,
    pub(crate) classification: LocalFailoverClassification,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct LocalAdaptiveRateLimitEffect<'a> {
    pub(crate) status_code: u16,
    pub(crate) classification: LocalFailoverClassification,
    pub(crate) headers: Option<&'a BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct LocalHealthFailureEffect {
    pub(crate) status_code: u16,
    pub(crate) classification: LocalFailoverClassification,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct LocalHealthSuccessEffect;

#[derive(Debug, Clone, Copy)]
pub(crate) struct LocalAdaptiveSuccessEffect;

#[derive(Debug, Clone, Copy)]
pub(crate) struct LocalOAuthInvalidationEffect<'a> {
    pub(crate) status_code: u16,
    pub(crate) response_text: Option<&'a str>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum LocalExecutionEffect<'a> {
    AttemptFailure(LocalAttemptFailureEffect),
    AdaptiveRateLimit(LocalAdaptiveRateLimitEffect<'a>),
    HealthFailure(LocalHealthFailureEffect),
    HealthSuccess(LocalHealthSuccessEffect),
    AdaptiveSuccess(LocalAdaptiveSuccessEffect),
    OauthInvalidation(LocalOAuthInvalidationEffect<'a>),
    PoolSuccessSync {
        payload: &'a GatewaySyncReportRequest,
    },
    PoolSuccessStream {
        payload: &'a GatewayStreamReportRequest,
    },
    PoolError(LocalPoolErrorEffect<'a>),
    PoolStreamTimeout,
}

struct PoolFeedbackContext {
    pool_config: AdminProviderPoolConfig,
    sticky_session_token: Option<String>,
}

const ADAPTIVE_RPM_RECENT_CANDIDATE_LIMIT: usize = 512;
const LOCAL_EXECUTION_SCHEDULER_AFFINITY_MAX_ENTRIES: usize = 10_000;

pub(crate) async fn apply_local_execution_effect(
    state: &AppState,
    context: LocalExecutionEffectContext<'_>,
    effect: LocalExecutionEffect<'_>,
) {
    match effect {
        LocalExecutionEffect::AttemptFailure(effect) => {
            record_attempt_failure_effect(state, context, effect).await;
        }
        LocalExecutionEffect::AdaptiveRateLimit(effect) => {
            record_adaptive_rate_limit_effect(state, context, effect).await;
        }
        LocalExecutionEffect::HealthFailure(effect) => {
            record_health_failure_effect(state, context, effect).await;
        }
        LocalExecutionEffect::HealthSuccess(effect) => {
            record_health_success_effect(state, context, effect).await;
        }
        LocalExecutionEffect::AdaptiveSuccess(effect) => {
            record_adaptive_success_effect(state, context, effect).await;
        }
        LocalExecutionEffect::OauthInvalidation(effect) => {
            record_oauth_invalidation_effect(state, context, effect).await;
        }
        LocalExecutionEffect::PoolSuccessSync { payload } => {
            record_sync_pool_success_effect(state, context, payload).await;
            release_pool_key_lease_effect(state, context).await;
        }
        LocalExecutionEffect::PoolSuccessStream { payload } => {
            record_stream_pool_success_effect(state, context, payload).await;
            release_pool_key_lease_effect(state, context).await;
        }
        LocalExecutionEffect::PoolError(effect) => {
            record_pool_error_effect(state, context, effect).await;
            release_pool_key_lease_effect(state, context).await;
        }
        LocalExecutionEffect::PoolStreamTimeout => {
            record_pool_stream_timeout_effect(state, context).await;
            release_pool_key_lease_effect(state, context).await;
        }
    }
}

async fn release_pool_key_lease_effect(state: &AppState, context: LocalExecutionEffectContext<'_>) {
    let metadata = local_execution_candidate_metadata_from_report_context(context.report_context);
    let Some(lease) = metadata.pool_key_lease else {
        return;
    };
    if let Err(err) =
        release_admin_provider_pool_key_lease(state.runtime_state.as_ref(), &lease).await
    {
        warn!(
            error = ?err,
            provider_id = %context.plan.provider_id,
            key_id = %context.plan.key_id,
            "gateway orchestration effects: failed to release pool key lease"
        );
    }
}

fn report_context_string_field<'a>(
    report_context: Option<&'a Value>,
    field: &str,
) -> Option<&'a str> {
    report_context
        .and_then(|context| context.get(field))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn local_scheduler_affinity_cache_key(report_context: Option<&Value>) -> Option<String> {
    let client_session_affinity = local_client_session_affinity(report_context);
    build_scheduler_affinity_cache_key_for_api_key_id_with_client_session(
        report_context_string_field(report_context, "api_key_id")?,
        report_context_string_field(report_context, "client_api_format")?,
        report_context_string_field(report_context, "model")?,
        client_session_affinity.as_ref(),
    )
}

fn local_client_session_affinity(report_context: Option<&Value>) -> Option<ClientSessionAffinity> {
    let report_context = report_context?;
    if let Some(affinity) = client_session_affinity_from_report_context_value(
        report_context.get(CLIENT_SESSION_AFFINITY_REPORT_CONTEXT_FIELD),
    ) {
        return Some(affinity);
    }

    let headers = header_map_from_report_context(report_context.get("original_headers"));
    let body_json = report_context
        .get("original_request_body")
        .filter(|value| !value.is_null());

    crate::client_session_affinity::client_session_affinity_from_request(&headers, body_json)
}

fn header_map_from_report_context(headers: Option<&Value>) -> http::HeaderMap {
    let mut header_map = http::HeaderMap::new();
    let Some(headers) = headers.and_then(Value::as_object) else {
        return header_map;
    };

    for (name, value) in headers {
        let Some(value) = value.as_str() else {
            continue;
        };
        let Ok(name) = http::header::HeaderName::from_bytes(name.as_bytes()) else {
            continue;
        };
        let Ok(value) = http::HeaderValue::from_str(value) else {
            continue;
        };
        header_map.insert(name, value);
    }

    header_map
}

fn local_scheduler_affinity_target(plan: &ExecutionPlan) -> Option<SchedulerAffinityTarget> {
    let provider_id = plan.provider_id.trim();
    let endpoint_id = plan.endpoint_id.trim();
    let key_id = plan.key_id.trim();
    if provider_id.is_empty() || endpoint_id.is_empty() || key_id.is_empty() {
        return None;
    }

    Some(SchedulerAffinityTarget {
        provider_id: provider_id.to_string(),
        endpoint_id: endpoint_id.to_string(),
        key_id: key_id.to_string(),
    })
}

async fn local_execution_plan_uses_pool(state: &AppState, plan: &ExecutionPlan) -> bool {
    let Ok(Some(transport)) = state
        .read_provider_transport_snapshot(&plan.provider_id, &plan.endpoint_id, &plan.key_id)
        .await
    else {
        return false;
    };

    admin_provider_pool_config_from_config_value(transport.provider.config.as_ref()).is_some()
}

async fn local_scheduler_affinity_matches_failed_target(
    state: &AppState,
    plan: &ExecutionPlan,
    cached_target: &SchedulerAffinityTarget,
    failed_target: &SchedulerAffinityTarget,
) -> bool {
    if cached_target == failed_target {
        return true;
    }
    if cached_target.provider_id != failed_target.provider_id
        || cached_target.endpoint_id != failed_target.endpoint_id
    {
        return false;
    }

    local_execution_plan_uses_pool(state, plan).await
}

async fn scheduler_cache_affinity_enabled(state: &AppState) -> bool {
    match read_scheduler_ordering_config(state).await {
        Ok(config) => config.scheduling_mode == SchedulerSchedulingMode::CacheAffinity,
        Err(error) => {
            warn!(
                event_name = "orchestration_scheduler_affinity_config_load_failed",
                log_type = "event",
                error = ?error,
                "failed to load scheduler config while checking cache affinity mode"
            );
            SchedulerSchedulingMode::default() == SchedulerSchedulingMode::CacheAffinity
        }
    }
}

async fn remember_successful_local_scheduler_affinity(
    state: &AppState,
    context: LocalExecutionEffectContext<'_>,
) {
    if !scheduler_cache_affinity_enabled(state).await {
        return;
    }
    let Some(cache_key) = local_scheduler_affinity_cache_key(context.report_context) else {
        return;
    };
    let Some(target) = local_scheduler_affinity_target(context.plan) else {
        return;
    };
    let expected_epoch =
        local_execution_candidate_metadata_from_report_context(context.report_context)
            .scheduler_affinity_epoch;

    let _ = state.remember_scheduler_affinity_target_for_epoch(
        &cache_key,
        target,
        SCHEDULER_AFFINITY_TTL,
        LOCAL_EXECUTION_SCHEDULER_AFFINITY_MAX_ENTRIES,
        expected_epoch,
    );
}

fn pool_feedback_request_body<'a>(
    plan: &'a ExecutionPlan,
    report_context: Option<&'a Value>,
) -> Option<&'a Value> {
    report_context
        .and_then(Value::as_object)
        .and_then(|object| object.get("original_request_body"))
        .filter(|value| !value.is_null())
        .or(plan.body.json_body.as_ref())
}

async fn resolve_pool_feedback_context(
    state: &AppState,
    context: LocalExecutionEffectContext<'_>,
) -> Option<PoolFeedbackContext> {
    let plan = context.plan;
    let transport = match state
        .read_provider_transport_snapshot(&plan.provider_id, &plan.endpoint_id, &plan.key_id)
        .await
    {
        Ok(Some(transport)) => transport,
        Ok(None) => return None,
        Err(err) => {
            warn!(
                "gateway orchestration effects: failed to read transport snapshot for provider {} endpoint {} key {}: {:?}",
                plan.provider_id, plan.endpoint_id, plan.key_id, err
            );
            return None;
        }
    };

    let Some(pool_config) =
        admin_provider_pool_config_from_config_value(transport.provider.config.as_ref())
    else {
        return None;
    };

    let sticky_session_token = pool_feedback_request_body(plan, context.report_context)
        .and_then(extract_pool_sticky_session_token);

    Some(PoolFeedbackContext {
        pool_config,
        sticky_session_token,
    })
}

fn total_tokens_used(outcome: &TerminalUsageOutcome) -> u64 {
    outcome
        .standardized_usage
        .as_ref()
        .map(|usage| {
            usage
                .input_tokens
                .saturating_add(usage.output_tokens)
                .max(0) as u64
        })
        .unwrap_or(0)
}

fn resolve_ttfb_ms(telemetry: Option<&ExecutionTelemetry>) -> Option<u64> {
    telemetry.and_then(|telemetry| telemetry.ttfb_ms.or(telemetry.elapsed_ms))
}

async fn record_attempt_failure_effect(
    state: &AppState,
    context: LocalExecutionEffectContext<'_>,
    effect: LocalAttemptFailureEffect,
) {
    if !local_candidate_failure_should_invalidate_affinity(
        effect.classification,
        effect.status_code,
    ) {
        return;
    }

    if let Some(cache_key) = local_scheduler_affinity_cache_key(context.report_context) {
        let Some(failed_target) = local_scheduler_affinity_target(context.plan) else {
            return;
        };
        let Some(cached_target) =
            state.read_scheduler_affinity_target(&cache_key, SCHEDULER_AFFINITY_TTL)
        else {
            return;
        };
        if local_scheduler_affinity_matches_failed_target(
            state,
            context.plan,
            &cached_target,
            &failed_target,
        )
        .await
        {
            let _ = state.remove_scheduler_affinity_cache_entry(&cache_key);
        }
    }
}

async fn record_sync_pool_success_effect(
    state: &AppState,
    context: LocalExecutionEffectContext<'_>,
    payload: &GatewaySyncReportRequest,
) {
    let Some(pool_context) = resolve_pool_feedback_context(state, context).await else {
        return;
    };

    let usage_outcome =
        build_sync_terminal_usage_outcome(context.plan, context.report_context, payload);
    record_admin_provider_pool_success(
        state.runtime_state.as_ref(),
        &context.plan.provider_id,
        &context.plan.key_id,
        &pool_context.pool_config,
        pool_context.sticky_session_token.as_deref(),
        total_tokens_used(&usage_outcome),
        resolve_ttfb_ms(payload.telemetry.as_ref()),
    )
    .await;
    record_pool_score_schedule_feedback(
        state,
        context,
        Some(true),
        Some(PoolMemberHardState::Available),
        Some(50),
        serde_json::json!({
            "last_request_feedback": {
                "source": "sync_success"
            }
        }),
    )
    .await;
}

async fn record_adaptive_rate_limit_effect(
    state: &AppState,
    context: LocalExecutionEffectContext<'_>,
    effect: LocalAdaptiveRateLimitEffect<'_>,
) {
    let observed_at_unix_secs = current_unix_secs();
    let Some(current_key) = state
        .read_provider_catalog_keys_by_ids(std::slice::from_ref(&context.plan.key_id))
        .await
        .ok()
        .and_then(|mut keys| keys.drain(..).next())
    else {
        return;
    };
    let current_rpm = state
        .read_recent_request_candidates(ADAPTIVE_RPM_RECENT_CANDIDATE_LIMIT)
        .await
        .ok()
        .map(|recent_candidates| {
            count_recent_rpm_requests_for_provider_key(
                &recent_candidates,
                &context.plan.key_id,
                observed_at_unix_secs,
            ) as u32
        });
    let Some(projection) = project_local_adaptive_rate_limit(
        &current_key,
        effect.classification,
        effect.status_code,
        current_rpm,
        effect.headers,
        observed_at_unix_secs,
    ) else {
        return;
    };

    let mut updated_key = current_key.clone();
    updated_key.rpm_429_count = Some(projection.rpm_429_count);
    updated_key.learned_rpm_limit = projection.learned_rpm_limit;
    updated_key.last_429_at_unix_secs = Some(projection.last_429_at_unix_secs);
    updated_key.last_429_type = Some(projection.last_429_type);
    updated_key.adjustment_history = projection.adjustment_history;
    updated_key.utilization_samples = projection.utilization_samples;
    updated_key.last_probe_increase_at_unix_secs = projection.last_probe_increase_at_unix_secs;
    updated_key.last_rpm_peak = projection.last_rpm_peak;
    updated_key.status_snapshot = Some(projection.status_snapshot);
    updated_key.updated_at_unix_secs = Some(observed_at_unix_secs);

    if let Err(err) = state
        .update_provider_catalog_key_runtime_state(&updated_key)
        .await
    {
        warn!(
            "gateway orchestration effects: failed to persist adaptive rate-limit projection for provider {} endpoint {} key {}: {:?}",
            context.plan.provider_id, context.plan.endpoint_id, context.plan.key_id, err
        );
    }
}

async fn record_adaptive_success_effect(
    state: &AppState,
    context: LocalExecutionEffectContext<'_>,
    _effect: LocalAdaptiveSuccessEffect,
) {
    let observed_at_unix_secs = current_unix_secs();
    let Some(current_key) = state
        .read_provider_catalog_keys_by_ids(std::slice::from_ref(&context.plan.key_id))
        .await
        .ok()
        .and_then(|mut keys| keys.drain(..).next())
    else {
        return;
    };
    let Some(recent_candidates) = state
        .read_recent_request_candidates(ADAPTIVE_RPM_RECENT_CANDIDATE_LIMIT)
        .await
        .ok()
    else {
        return;
    };
    let current_rpm = count_recent_rpm_requests_for_provider_key(
        &recent_candidates,
        &context.plan.key_id,
        observed_at_unix_secs,
    ) as u32;
    let Some(projection) =
        project_local_adaptive_success(&current_key, current_rpm, observed_at_unix_secs)
    else {
        return;
    };

    let mut updated_key = current_key.clone();
    updated_key.learned_rpm_limit = projection.learned_rpm_limit;
    updated_key.adjustment_history = projection.adjustment_history;
    updated_key.utilization_samples = projection.utilization_samples;
    updated_key.last_probe_increase_at_unix_secs = projection.last_probe_increase_at_unix_secs;
    updated_key.status_snapshot = Some(projection.status_snapshot);
    updated_key.updated_at_unix_secs = Some(observed_at_unix_secs);

    if let Err(err) = state
        .update_provider_catalog_key_runtime_state(&updated_key)
        .await
    {
        warn!(
            "gateway orchestration effects: failed to persist adaptive success projection for provider {} endpoint {} key {}: {:?}",
            context.plan.provider_id, context.plan.endpoint_id, context.plan.key_id, err
        );
    }
}

async fn record_health_failure_effect(
    state: &AppState,
    context: LocalExecutionEffectContext<'_>,
    effect: LocalHealthFailureEffect,
) {
    let api_format = context.plan.provider_api_format.trim();
    if api_format.is_empty() {
        return;
    }

    let Some(current_key) = state
        .read_provider_catalog_keys_by_ids(std::slice::from_ref(&context.plan.key_id))
        .await
        .ok()
        .and_then(|mut keys| keys.drain(..).next())
    else {
        return;
    };
    let is_pool_provider = local_execution_plan_uses_pool(state, context.plan).await;
    let observed_at_unix_secs = current_unix_secs();
    let Some(health_by_format) = project_local_failure_health(
        current_key.health_by_format.as_ref(),
        api_format,
        effect.classification,
        effect.status_code,
        observed_at_unix_secs,
    ) else {
        return;
    };
    let consecutive_failures = health_by_format
        .get(api_format)
        .and_then(|value| value.get("consecutive_failures"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let circuit_breaker_update_owned = if is_pool_provider {
        None
    } else {
        project_local_key_circuit_failure(
            current_key.circuit_breaker_by_format.as_ref(),
            api_format,
            observed_at_unix_secs,
            consecutive_failures,
            current_key.max_probe_interval_minutes,
        )
    };
    let circuit_breaker_update = if is_pool_provider {
        None
    } else {
        circuit_breaker_update_owned
            .as_ref()
            .or(current_key.circuit_breaker_by_format.as_ref())
    };

    if let Err(err) = state
        .update_provider_catalog_key_health_state(
            &context.plan.key_id,
            current_key.is_active,
            Some(&health_by_format),
            circuit_breaker_update,
        )
        .await
    {
        warn!(
            "gateway orchestration effects: failed to persist health failure projection for provider {} endpoint {} key {}: {:?}",
            context.plan.provider_id, context.plan.endpoint_id, context.plan.key_id, err
        );
    }
}

async fn record_health_success_effect(
    state: &AppState,
    context: LocalExecutionEffectContext<'_>,
    _effect: LocalHealthSuccessEffect,
) {
    remember_successful_local_scheduler_affinity(state, context).await;

    let api_format = context.plan.provider_api_format.trim();
    if api_format.is_empty() {
        return;
    }

    let Some(current_key) = state
        .read_provider_catalog_keys_by_ids(std::slice::from_ref(&context.plan.key_id))
        .await
        .ok()
        .and_then(|mut keys| keys.drain(..).next())
    else {
        return;
    };
    let is_pool_provider = local_execution_plan_uses_pool(state, context.plan).await;
    let Some(health_by_format) =
        project_local_success_health(current_key.health_by_format.as_ref(), api_format)
    else {
        return;
    };
    let circuit_breaker_update_owned = if is_pool_provider {
        None
    } else {
        current_key
            .circuit_breaker_by_format
            .as_ref()
            .and_then(|current| project_local_key_circuit_closed(Some(current), api_format))
    };
    if current_key.health_by_format.as_ref() == Some(&health_by_format)
        && ((is_pool_provider && current_key.circuit_breaker_by_format.is_none())
            || (!is_pool_provider
                && circuit_breaker_update_owned.as_ref()
                    == current_key.circuit_breaker_by_format.as_ref()))
    {
        return;
    }
    let circuit_breaker_update = if is_pool_provider {
        None
    } else {
        circuit_breaker_update_owned
            .as_ref()
            .or(current_key.circuit_breaker_by_format.as_ref())
    };

    if let Err(err) = state
        .update_provider_catalog_key_health_state(
            &context.plan.key_id,
            current_key.is_active,
            Some(&health_by_format),
            circuit_breaker_update,
        )
        .await
    {
        warn!(
            "gateway orchestration effects: failed to persist health success projection for provider {} endpoint {} key {}: {:?}",
            context.plan.provider_id, context.plan.endpoint_id, context.plan.key_id, err
        );
    }
}

async fn record_stream_pool_success_effect(
    state: &AppState,
    context: LocalExecutionEffectContext<'_>,
    payload: &GatewayStreamReportRequest,
) {
    let Some(pool_context) = resolve_pool_feedback_context(state, context).await else {
        return;
    };

    let usage_outcome =
        build_stream_terminal_usage_outcome(context.plan, context.report_context, payload);
    record_admin_provider_pool_success(
        state.runtime_state.as_ref(),
        &context.plan.provider_id,
        &context.plan.key_id,
        &pool_context.pool_config,
        pool_context.sticky_session_token.as_deref(),
        total_tokens_used(&usage_outcome),
        resolve_ttfb_ms(payload.telemetry.as_ref()),
    )
    .await;
    record_pool_score_schedule_feedback(
        state,
        context,
        Some(true),
        Some(PoolMemberHardState::Available),
        Some(50),
        serde_json::json!({
            "last_request_feedback": {
                "source": "stream_success"
            }
        }),
    )
    .await;
}

async fn record_pool_error_effect(
    state: &AppState,
    context: LocalExecutionEffectContext<'_>,
    effect: LocalPoolErrorEffect<'_>,
) {
    let terminal_error_reason =
        admin_provider_pool_key_terminal_error_reason(effect.status_code, effect.error_body);
    if terminal_error_reason.is_none()
        && !local_candidate_failure_should_record_pool_error(
            effect.classification,
            effect.status_code,
        )
    {
        return;
    }

    let Some(pool_context) = resolve_pool_feedback_context(state, context).await else {
        return;
    };

    clear_pool_key_circuit_breaker(state, context).await;
    record_admin_provider_pool_error(
        state.runtime_state.as_ref(),
        &context.plan.provider_id,
        &context.plan.key_id,
        &pool_context.pool_config,
        effect.status_code,
        effect.error_body,
        Some(effect.headers),
    )
    .await;
    record_pool_score_schedule_feedback(
        state,
        context,
        Some(false),
        pool_score_hard_state_for_status(effect.status_code, effect.error_body),
        Some(pool_score_delta_for_status(effect.status_code)),
        serde_json::json!({
            "last_request_feedback": {
                "source": "pool_error",
                "status_code": effect.status_code,
                "classification": format!("{:?}", effect.classification)
            }
        }),
    )
    .await;
}

async fn clear_pool_key_circuit_breaker(
    state: &AppState,
    context: LocalExecutionEffectContext<'_>,
) {
    let Some(current_key) = state
        .read_provider_catalog_keys_by_ids(std::slice::from_ref(&context.plan.key_id))
        .await
        .ok()
        .and_then(|mut keys| keys.drain(..).next())
    else {
        return;
    };
    if current_key.circuit_breaker_by_format.is_none() {
        return;
    }

    if let Err(err) = state
        .update_provider_catalog_key_health_state(
            &context.plan.key_id,
            current_key.is_active,
            current_key.health_by_format.as_ref(),
            None,
        )
        .await
    {
        warn!(
            "gateway orchestration effects: failed to clear pool key circuit for provider {} endpoint {} key {}: {:?}",
            context.plan.provider_id, context.plan.endpoint_id, context.plan.key_id, err
        );
    }
}

async fn record_oauth_invalidation_effect(
    state: &AppState,
    context: LocalExecutionEffectContext<'_>,
    effect: LocalOAuthInvalidationEffect<'_>,
) {
    if effect.status_code < 400 {
        return;
    }

    let plan = context.plan;
    let transport = match state
        .read_provider_transport_snapshot(&plan.provider_id, &plan.endpoint_id, &plan.key_id)
        .await
    {
        Ok(Some(transport)) => transport,
        Ok(None) => return,
        Err(err) => {
            warn!(
                "gateway orchestration effects: failed to read transport snapshot for oauth invalidation provider {} endpoint {} key {}: {:?}",
                plan.provider_id, plan.endpoint_id, plan.key_id, err
            );
            return;
        }
    };
    if !transport.key.auth_type.trim().eq_ignore_ascii_case("oauth") {
        return;
    }

    let Some(invalid_reason) = resolve_local_oauth_invalid_reason(
        transport.provider.provider_type.as_str(),
        effect.status_code,
        effect.response_text,
    ) else {
        return;
    };

    if let Err(err) = state
        .mark_provider_catalog_key_oauth_invalid(
            &plan.key_id,
            transport.provider.provider_type.as_str(),
            invalid_reason.as_str(),
        )
        .await
    {
        warn!(
            "gateway orchestration effects: failed to persist oauth invalidation for provider {} endpoint {} key {}: {:?}",
            plan.provider_id, plan.endpoint_id, plan.key_id, err
        );
    }
    record_pool_score_schedule_feedback(
        state,
        context,
        Some(false),
        Some(PoolMemberHardState::AuthInvalid),
        Some(-2_000),
        serde_json::json!({
            "last_request_feedback": {
                "source": "oauth_invalidation",
                "status_code": effect.status_code,
                "reason": invalid_reason
            }
        }),
    )
    .await;
}

fn resolve_local_oauth_invalid_reason(
    provider_type: &str,
    status_code: u16,
    response_text: Option<&str>,
) -> Option<String> {
    let upstream_message = local_failover_error_message(response_text);
    match provider_type.trim().to_ascii_lowercase().as_str() {
        "codex" => admin_provider_quota_pure::codex_runtime_invalid_reason(
            status_code,
            upstream_message.as_deref(),
        ),
        _ => None,
    }
}

fn local_candidate_failure_should_invalidate_affinity(
    classification: LocalFailoverClassification,
    status_code: u16,
) -> bool {
    if status_code < 400 {
        return false;
    }

    match classification {
        LocalFailoverClassification::RetrySuccessPattern
        | LocalFailoverClassification::RetryStatusCode
        | LocalFailoverClassification::RetryUpstreamFailure => true,
        LocalFailoverClassification::UseDefault | LocalFailoverClassification::StopStatusCode => {
            status_code >= 500
        }
        LocalFailoverClassification::StopErrorPattern
        | LocalFailoverClassification::StopExecutionError => false,
    }
}

fn local_candidate_failure_should_record_pool_error(
    classification: LocalFailoverClassification,
    status_code: u16,
) -> bool {
    if status_code == 400 {
        return false;
    }

    local_candidate_failure_should_invalidate_affinity(classification, status_code)
}

async fn record_pool_stream_timeout_effect(
    state: &AppState,
    context: LocalExecutionEffectContext<'_>,
) {
    let Some(pool_context) = resolve_pool_feedback_context(state, context).await else {
        return;
    };

    record_admin_provider_pool_stream_timeout(
        state.runtime_state.as_ref(),
        &context.plan.provider_id,
        &context.plan.key_id,
        &pool_context.pool_config,
    )
    .await;
    record_pool_score_schedule_feedback(
        state,
        context,
        Some(false),
        Some(PoolMemberHardState::Cooldown),
        Some(-250),
        serde_json::json!({
            "last_request_feedback": {
                "source": "stream_timeout"
            }
        }),
    )
    .await;
}

async fn record_pool_score_schedule_feedback(
    state: &AppState,
    context: LocalExecutionEffectContext<'_>,
    succeeded: Option<bool>,
    hard_state: Option<PoolMemberHardState>,
    score_delta: Option<i32>,
    score_reason_patch: Value,
) {
    if context.plan.provider_id.trim().is_empty() || context.plan.key_id.trim().is_empty() {
        return;
    }
    let feedback = PoolMemberScheduleFeedback {
        identity: PoolMemberIdentity::provider_api_key(
            context.plan.provider_id.clone(),
            context.plan.key_id.clone(),
        ),
        scope: None,
        scheduled_at: current_unix_secs(),
        succeeded,
        hard_state,
        score_delta,
        score_reason_patch: Some(score_reason_patch),
    };
    if let Err(err) = state
        .data
        .record_pool_member_schedule_feedback(feedback)
        .await
    {
        warn!(
            provider_id = %context.plan.provider_id,
            key_id = %context.plan.key_id,
            error = ?err,
            "gateway orchestration effects: failed to record pool score schedule feedback"
        );
    }
}

fn pool_score_hard_state_for_status(
    status_code: u16,
    error_body: Option<&str>,
) -> Option<PoolMemberHardState> {
    if let Some(reason) = admin_provider_pool_key_terminal_error_reason(status_code, error_body) {
        return Some(pool_score_hard_state_for_terminal_error_reason(&reason));
    }

    match status_code {
        401 | 403 => Some(PoolMemberHardState::AuthInvalid),
        402 => Some(PoolMemberHardState::QuotaExhausted),
        429 | 500..=599 => Some(PoolMemberHardState::Cooldown),
        _ => {
            let body = error_body.unwrap_or_default().to_ascii_lowercase();
            if body.contains("quota") && body.contains("exceed") {
                Some(PoolMemberHardState::QuotaExhausted)
            } else if body.contains("invalid") && body.contains("token") {
                Some(PoolMemberHardState::AuthInvalid)
            } else if body.contains("banned")
                || body.contains("suspended")
                || body.contains("blocked")
            {
                Some(PoolMemberHardState::Banned)
            } else {
                None
            }
        }
    }
}

fn pool_score_hard_state_for_terminal_error_reason(reason: &str) -> PoolMemberHardState {
    if reason.starts_with("payment_required_") {
        PoolMemberHardState::QuotaExhausted
    } else if reason.starts_with("forbidden_") {
        PoolMemberHardState::AuthInvalid
    } else {
        PoolMemberHardState::Banned
    }
}

fn pool_score_delta_for_status(status_code: u16) -> i32 {
    match status_code {
        401 | 403 => -2_000,
        402 => -1_000,
        429 => -500,
        500..=599 => -300,
        _ => -100,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use aether_contracts::{ExecutionPlan, RequestBody};
    use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
    use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data_contracts::repository::candidates::{
        RequestCandidateStatus, StoredRequestCandidate,
    };
    use aether_data_contracts::repository::pool_scores::PoolMemberHardState;
    use aether_data_contracts::repository::provider_catalog::{
        StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
    };
    use aether_testkit::ManagedRedisServer;
    use serde_json::{json, Value};

    use super::{
        apply_local_execution_effect, local_candidate_failure_should_record_pool_error,
        pool_score_hard_state_for_status, LocalAdaptiveRateLimitEffect, LocalAdaptiveSuccessEffect,
        LocalAttemptFailureEffect, LocalExecutionEffect, LocalExecutionEffectContext,
        LocalHealthFailureEffect, LocalHealthSuccessEffect, LocalOAuthInvalidationEffect,
        LocalPoolErrorEffect,
    };
    use crate::data::{GatewayDataConfig, GatewayDataState};
    use crate::orchestration::LocalFailoverClassification;
    use crate::scheduler::affinity::SCHEDULER_AFFINITY_TTL;
    use crate::AppState;
    use aether_scheduler_core::{
        build_scheduler_affinity_cache_key_for_api_key_id,
        build_scheduler_affinity_cache_key_for_api_key_id_with_client_session,
        ClientSessionAffinity, SchedulerAffinityTarget,
    };

    async fn start_managed_redis_or_skip() -> Option<ManagedRedisServer> {
        match ManagedRedisServer::start().await {
            Ok(server) => Some(server),
            Err(err) if err.to_string().contains("No such file or directory") => {
                eprintln!("skipping redis-backed orchestration effect test: {err}");
                None
            }
            Err(err) => panic!("redis server should start: {err}"),
        }
    }

    fn sample_plan() -> ExecutionPlan {
        ExecutionPlan {
            request_id: "req-1".to_string(),
            candidate_id: Some("cand-1".to_string()),
            provider_name: Some("openai".to_string()),
            provider_id: "prov-1".to_string(),
            endpoint_id: "ep-1".to_string(),
            key_id: "key-1".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/v1/chat/completions".to_string(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"model":"gpt-5"})),
            stream: false,
            client_api_format: "openai:chat".to_string(),
            provider_api_format: "openai:chat".to_string(),
            model_name: Some("gpt-5".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        }
    }

    fn session_affinity() -> ClientSessionAffinity {
        ClientSessionAffinity::new(
            Some("generic".to_string()),
            Some("session=session-1;agent=coder".to_string()),
        )
    }

    fn session_report_context() -> Value {
        json!({
            "api_key_id": "api-key-1",
            "client_api_format": "openai:chat",
            "model": "gpt-5",
            "client_session_affinity": {
                "client_family": "generic",
                "session_key": "session=session-1;agent=coder"
            },
            "original_headers": {
                "x-aether-session-id": "raw-session",
                "x-aether-agent-id": "raw-agent"
            },
            "original_request_body": {
                "model": "gpt-5"
            }
        })
    }

    fn session_scheduler_affinity_cache_key() -> String {
        build_scheduler_affinity_cache_key_for_api_key_id_with_client_session(
            "api-key-1",
            "openai:chat",
            "gpt-5",
            Some(&session_affinity()),
        )
        .expect("session scheduler affinity cache key should build")
    }

    fn sample_codex_plan() -> ExecutionPlan {
        ExecutionPlan {
            request_id: "req-codex-1".to_string(),
            candidate_id: Some("cand-codex-1".to_string()),
            provider_name: Some("codex".to_string()),
            provider_id: "provider-codex-cli-local-1".to_string(),
            endpoint_id: "endpoint-codex-cli-local-1".to_string(),
            key_id: "key-codex-cli-local-1".to_string(),
            method: "POST".to_string(),
            url: "https://chatgpt.com/backend-api/codex".to_string(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"model":"gpt-5.4"})),
            stream: false,
            client_api_format: "openai:responses".to_string(),
            provider_api_format: "openai:responses".to_string(),
            model_name: Some("gpt-5.4".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        }
    }

    fn sample_codex_provider() -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            "provider-codex-cli-local-1".to_string(),
            "codex".to_string(),
            Some("https://chatgpt.com".to_string()),
            "codex".to_string(),
        )
        .expect("provider should build")
        .with_transport_fields(
            true,
            false,
            false,
            None,
            Some(2),
            None,
            Some(20.0),
            None,
            Some(json!({"pool_advanced": {}})),
        )
    }

    fn sample_codex_endpoint() -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            "endpoint-codex-cli-local-1".to_string(),
            "provider-codex-cli-local-1".to_string(),
            "openai:responses".to_string(),
            Some("openai".to_string()),
            Some("cli".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://chatgpt.com/backend-api/codex".to_string(),
            None,
            None,
            Some(2),
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport should build")
    }

    fn sample_codex_key() -> StoredProviderCatalogKey {
        let encrypted_auth_config = encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"codex","refresh_token":"rt-codex-local-123"}"#,
        )
        .expect("auth config should encrypt");
        StoredProviderCatalogKey::new(
            "key-codex-cli-local-1".to_string(),
            "provider-codex-cli-local-1".to_string(),
            "oauth".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(serde_json::json!(["openai:responses"])),
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "__placeholder__")
                .expect("placeholder api key should encrypt"),
            Some(encrypted_auth_config),
            None,
            Some(serde_json::json!({"openai:responses": 1})),
            None,
            None,
            None,
            None,
        )
        .expect("key transport should build")
    }

    fn codex_state() -> AppState {
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_codex_provider()],
            vec![sample_codex_endpoint()],
            vec![sample_codex_key()],
        ));
        AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(repository)
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
    }

    fn codex_state_with_redis(redis_url: &str, redis_key_prefix: &str) -> AppState {
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_codex_provider()],
            vec![sample_codex_endpoint()],
            vec![sample_codex_key()],
        ));
        let data_state = GatewayDataState::from_config(
            GatewayDataConfig::disabled()
                .with_redis_url(redis_url, Some(redis_key_prefix))
                .with_encryption_key(DEVELOPMENT_ENCRYPTION_KEY),
        )
        .expect("data state should build")
        .attach_provider_catalog_repository_for_tests(repository);
        AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(data_state)
    }

    fn sample_health_provider() -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            "prov-1".to_string(),
            "openai".to_string(),
            Some("https://example.com".to_string()),
            "custom".to_string(),
        )
        .expect("provider should build")
    }

    fn sample_pool_health_provider() -> StoredProviderCatalogProvider {
        sample_health_provider().with_transport_fields(
            true,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            Some(json!({"pool_advanced": {}})),
        )
    }

    fn sample_health_endpoint() -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            "ep-1".to_string(),
            "prov-1".to_string(),
            "openai:chat".to_string(),
            Some("openai".to_string()),
            Some("chat".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://example.com/v1/chat/completions".to_string(),
            None,
            None,
            Some(2),
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport should build")
    }

    fn sample_health_key() -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "prov-1".to_string(),
            "prod".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(serde_json::json!(["openai:chat"])),
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "sk-test")
                .expect("api key should encrypt"),
            None,
            None,
            Some(serde_json::json!({"openai:chat": 1})),
            None,
            None,
            None,
            None,
        )
        .expect("key transport should build")
    }

    fn health_state() -> AppState {
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_health_provider()],
            vec![sample_health_endpoint()],
            vec![sample_health_key()],
        ));
        AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(repository)
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
    }

    fn pool_health_state() -> AppState {
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_pool_health_provider()],
            vec![sample_health_endpoint()],
            vec![sample_health_key()],
        ));
        AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(repository)
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
    }

    fn health_state_with_key(key: StoredProviderCatalogKey) -> AppState {
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_health_provider()],
            vec![sample_health_endpoint()],
            vec![key],
        ));
        AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(repository)
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
    }

    fn sample_adaptive_key() -> StoredProviderCatalogKey {
        let mut key = sample_health_key();
        key.name = "adaptive".to_string();
        key.rpm_limit = None;
        key.learned_rpm_limit = Some(12);
        key.rpm_429_count = Some(1);
        key
    }

    fn adaptive_state() -> AppState {
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_health_provider()],
            vec![sample_health_endpoint()],
            vec![sample_adaptive_key()],
        ));
        AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(repository)
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
    }

    fn adaptive_state_with_request_candidates(
        key: StoredProviderCatalogKey,
        request_candidates: Vec<StoredRequestCandidate>,
    ) -> AppState {
        let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_health_provider()],
            vec![sample_health_endpoint()],
            vec![key],
        ));
        let request_candidates =
            Arc::new(InMemoryRequestCandidateRepository::seed(request_candidates));
        AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(provider_catalog)
                    .with_request_candidate_reader(request_candidates)
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
    }

    fn fixed_limit_state() -> AppState {
        let mut key = sample_health_key();
        key.rpm_limit = Some(24);

        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_health_provider()],
            vec![sample_health_endpoint()],
            vec![key],
        ));
        AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(repository)
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
    }

    #[tokio::test]
    async fn attempt_failure_invalidates_scheduler_affinity_cache() {
        let state = AppState::new().expect("gateway state should build");
        let plan = sample_plan();
        let report_context = json!({
            "api_key_id": "api-key-1",
            "client_api_format": "openai:chat",
            "model": "gpt-5",
        });
        let cache_key =
            build_scheduler_affinity_cache_key_for_api_key_id("api-key-1", "openai:chat", "gpt-5")
                .expect("scheduler affinity cache key should build");

        state.remember_scheduler_affinity_target(
            &cache_key,
            SchedulerAffinityTarget {
                provider_id: "prov-1".to_string(),
                endpoint_id: "ep-1".to_string(),
                key_id: "key-1".to_string(),
            },
            SCHEDULER_AFFINITY_TTL,
            16,
        );
        assert!(state
            .read_scheduler_affinity_target(cache_key.as_str(), SCHEDULER_AFFINITY_TTL)
            .is_some());

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: Some(&report_context),
            },
            LocalExecutionEffect::AttemptFailure(LocalAttemptFailureEffect {
                status_code: 429,
                classification: LocalFailoverClassification::RetryUpstreamFailure,
            }),
        )
        .await;

        assert!(state
            .read_scheduler_affinity_target(cache_key.as_str(), SCHEDULER_AFFINITY_TTL)
            .is_none());
    }

    #[tokio::test]
    async fn attempt_failure_invalidates_session_scoped_scheduler_affinity_cache() {
        let state = AppState::new().expect("gateway state should build");
        let plan = sample_plan();
        let report_context = session_report_context();
        let session_cache_key = session_scheduler_affinity_cache_key();
        let legacy_cache_key =
            build_scheduler_affinity_cache_key_for_api_key_id("api-key-1", "openai:chat", "gpt-5")
                .expect("legacy scheduler affinity cache key should build");

        for cache_key in [&session_cache_key, &legacy_cache_key] {
            state.remember_scheduler_affinity_target(
                cache_key.as_str(),
                SchedulerAffinityTarget {
                    provider_id: "prov-1".to_string(),
                    endpoint_id: "ep-1".to_string(),
                    key_id: "key-1".to_string(),
                },
                SCHEDULER_AFFINITY_TTL,
                16,
            );
        }

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: Some(&report_context),
            },
            LocalExecutionEffect::AttemptFailure(LocalAttemptFailureEffect {
                status_code: 429,
                classification: LocalFailoverClassification::RetryUpstreamFailure,
            }),
        )
        .await;

        assert!(state
            .read_scheduler_affinity_target(session_cache_key.as_str(), SCHEDULER_AFFINITY_TTL)
            .is_none());
        assert!(state
            .read_scheduler_affinity_target(legacy_cache_key.as_str(), SCHEDULER_AFFINITY_TTL)
            .is_some());
    }

    #[tokio::test]
    async fn attempt_failure_keeps_scheduler_affinity_for_non_affinity_candidate() {
        let state = AppState::new().expect("gateway state should build");
        let plan = sample_plan();
        let report_context = json!({
            "api_key_id": "api-key-1",
            "client_api_format": "openai:chat",
            "model": "gpt-5",
        });
        let cache_key =
            build_scheduler_affinity_cache_key_for_api_key_id("api-key-1", "openai:chat", "gpt-5")
                .expect("scheduler affinity cache key should build");
        let affinity_target = SchedulerAffinityTarget {
            provider_id: "prov-2".to_string(),
            endpoint_id: "ep-2".to_string(),
            key_id: "key-2".to_string(),
        };

        state.remember_scheduler_affinity_target(
            &cache_key,
            affinity_target.clone(),
            SCHEDULER_AFFINITY_TTL,
            16,
        );

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: Some(&report_context),
            },
            LocalExecutionEffect::AttemptFailure(LocalAttemptFailureEffect {
                status_code: 524,
                classification: LocalFailoverClassification::RetryUpstreamFailure,
            }),
        )
        .await;

        assert_eq!(
            state.read_scheduler_affinity_target(cache_key.as_str(), SCHEDULER_AFFINITY_TTL),
            Some(affinity_target)
        );
    }

    #[tokio::test]
    async fn attempt_failure_keeps_scheduler_affinity_for_non_pool_sibling_key() {
        let state = health_state();
        let plan = sample_plan();
        let report_context = json!({
            "api_key_id": "api-key-1",
            "client_api_format": "openai:chat",
            "model": "gpt-5",
        });
        let cache_key =
            build_scheduler_affinity_cache_key_for_api_key_id("api-key-1", "openai:chat", "gpt-5")
                .expect("scheduler affinity cache key should build");
        let affinity_target = SchedulerAffinityTarget {
            provider_id: "prov-1".to_string(),
            endpoint_id: "ep-1".to_string(),
            key_id: "key-2".to_string(),
        };

        state.remember_scheduler_affinity_target(
            &cache_key,
            affinity_target.clone(),
            SCHEDULER_AFFINITY_TTL,
            16,
        );

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: Some(&report_context),
            },
            LocalExecutionEffect::AttemptFailure(LocalAttemptFailureEffect {
                status_code: 524,
                classification: LocalFailoverClassification::RetryUpstreamFailure,
            }),
        )
        .await;

        assert_eq!(
            state.read_scheduler_affinity_target(cache_key.as_str(), SCHEDULER_AFFINITY_TTL),
            Some(affinity_target)
        );
    }

    #[tokio::test]
    async fn attempt_failure_invalidates_scheduler_affinity_for_same_pool_candidate() {
        let state = pool_health_state();
        let plan = sample_plan();
        let report_context = json!({
            "api_key_id": "api-key-1",
            "client_api_format": "openai:chat",
            "model": "gpt-5",
        });
        let cache_key =
            build_scheduler_affinity_cache_key_for_api_key_id("api-key-1", "openai:chat", "gpt-5")
                .expect("scheduler affinity cache key should build");

        state.remember_scheduler_affinity_target(
            &cache_key,
            SchedulerAffinityTarget {
                provider_id: "prov-1".to_string(),
                endpoint_id: "ep-1".to_string(),
                key_id: "key-2".to_string(),
            },
            SCHEDULER_AFFINITY_TTL,
            16,
        );

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: Some(&report_context),
            },
            LocalExecutionEffect::AttemptFailure(LocalAttemptFailureEffect {
                status_code: 524,
                classification: LocalFailoverClassification::RetryUpstreamFailure,
            }),
        )
        .await;

        assert_eq!(
            state.read_scheduler_affinity_target(cache_key.as_str(), SCHEDULER_AFFINITY_TTL),
            None
        );
    }

    #[tokio::test]
    async fn attempt_failure_keeps_scheduler_affinity_for_non_failure_status() {
        let state = AppState::new().expect("gateway state should build");
        let plan = sample_plan();
        let report_context = json!({
            "api_key_id": "api-key-1",
            "client_api_format": "openai:chat",
            "model": "gpt-5",
        });
        let cache_key =
            build_scheduler_affinity_cache_key_for_api_key_id("api-key-1", "openai:chat", "gpt-5")
                .expect("scheduler affinity cache key should build");

        state.remember_scheduler_affinity_target(
            &cache_key,
            SchedulerAffinityTarget {
                provider_id: "prov-1".to_string(),
                endpoint_id: "ep-1".to_string(),
                key_id: "key-1".to_string(),
            },
            SCHEDULER_AFFINITY_TTL,
            16,
        );

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: Some(&report_context),
            },
            LocalExecutionEffect::AttemptFailure(LocalAttemptFailureEffect {
                status_code: 200,
                classification: LocalFailoverClassification::UseDefault,
            }),
        )
        .await;

        assert!(state
            .read_scheduler_affinity_target(cache_key.as_str(), SCHEDULER_AFFINITY_TTL)
            .is_some());
    }

    #[tokio::test]
    async fn configured_stop_pattern_keeps_scheduler_affinity_cache() {
        let state = AppState::new().expect("gateway state should build");
        let plan = sample_plan();
        let report_context = json!({
            "api_key_id": "api-key-1",
            "client_api_format": "openai:chat",
            "model": "gpt-5",
        });
        let cache_key =
            build_scheduler_affinity_cache_key_for_api_key_id("api-key-1", "openai:chat", "gpt-5")
                .expect("scheduler affinity cache key should build");

        state.remember_scheduler_affinity_target(
            &cache_key,
            SchedulerAffinityTarget {
                provider_id: "prov-1".to_string(),
                endpoint_id: "ep-1".to_string(),
                key_id: "key-1".to_string(),
            },
            SCHEDULER_AFFINITY_TTL,
            16,
        );

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: Some(&report_context),
            },
            LocalExecutionEffect::AttemptFailure(LocalAttemptFailureEffect {
                status_code: 400,
                classification: LocalFailoverClassification::StopErrorPattern,
            }),
        )
        .await;

        assert!(state
            .read_scheduler_affinity_target(cache_key.as_str(), SCHEDULER_AFFINITY_TTL)
            .is_some());
    }

    #[tokio::test]
    async fn success_remembers_scheduler_affinity_cache_for_final_candidate() {
        let state = AppState::new().expect("gateway state should build");
        let plan = sample_plan();
        let report_context = json!({
            "api_key_id": "api-key-1",
            "client_api_format": "openai:chat",
            "model": "gpt-5",
        });
        let cache_key =
            build_scheduler_affinity_cache_key_for_api_key_id("api-key-1", "openai:chat", "gpt-5")
                .expect("scheduler affinity cache key should build");

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: Some(&report_context),
            },
            LocalExecutionEffect::HealthSuccess(LocalHealthSuccessEffect),
        )
        .await;

        assert_eq!(
            state.read_scheduler_affinity_target(cache_key.as_str(), SCHEDULER_AFFINITY_TTL),
            Some(SchedulerAffinityTarget {
                provider_id: "prov-1".to_string(),
                endpoint_id: "ep-1".to_string(),
                key_id: "key-1".to_string(),
            })
        );
    }

    #[tokio::test]
    async fn health_success_keeps_scheduler_affinity_after_health_state_update() {
        let state = health_state();
        let plan = sample_plan();
        let report_context = json!({
            "api_key_id": "api-key-1",
            "client_api_format": "openai:chat",
            "model": "gpt-5",
        });
        let cache_key =
            build_scheduler_affinity_cache_key_for_api_key_id("api-key-1", "openai:chat", "gpt-5")
                .expect("scheduler affinity cache key should build");

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: Some(&report_context),
            },
            LocalExecutionEffect::HealthSuccess(LocalHealthSuccessEffect),
        )
        .await;

        assert_eq!(
            state.read_scheduler_affinity_target(cache_key.as_str(), SCHEDULER_AFFINITY_TTL),
            Some(SchedulerAffinityTarget {
                provider_id: "prov-1".to_string(),
                endpoint_id: "ep-1".to_string(),
                key_id: "key-1".to_string(),
            })
        );
    }

    #[tokio::test]
    async fn load_balance_success_does_not_remember_scheduler_affinity_cache() {
        let state = AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled().with_system_config_values_for_tests(vec![(
                    "scheduling_mode".to_string(),
                    json!("load_balance"),
                )]),
            );
        let plan = sample_plan();
        let report_context = json!({
            "api_key_id": "api-key-1",
            "client_api_format": "openai:chat",
            "model": "gpt-5",
        });
        let cache_key =
            build_scheduler_affinity_cache_key_for_api_key_id("api-key-1", "openai:chat", "gpt-5")
                .expect("scheduler affinity cache key should build");

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: Some(&report_context),
            },
            LocalExecutionEffect::HealthSuccess(LocalHealthSuccessEffect),
        )
        .await;

        assert!(state
            .read_scheduler_affinity_target(cache_key.as_str(), SCHEDULER_AFFINITY_TTL)
            .is_none());
    }

    #[tokio::test]
    async fn success_remembers_session_scoped_scheduler_affinity_cache() {
        let state = AppState::new().expect("gateway state should build");
        let plan = sample_plan();
        let report_context = session_report_context();
        let session_cache_key = session_scheduler_affinity_cache_key();
        let legacy_cache_key =
            build_scheduler_affinity_cache_key_for_api_key_id("api-key-1", "openai:chat", "gpt-5")
                .expect("legacy scheduler affinity cache key should build");

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: Some(&report_context),
            },
            LocalExecutionEffect::HealthSuccess(LocalHealthSuccessEffect),
        )
        .await;

        assert_eq!(
            state
                .read_scheduler_affinity_target(session_cache_key.as_str(), SCHEDULER_AFFINITY_TTL),
            Some(SchedulerAffinityTarget {
                provider_id: "prov-1".to_string(),
                endpoint_id: "ep-1".to_string(),
                key_id: "key-1".to_string(),
            })
        );
        assert!(state
            .read_scheduler_affinity_target(legacy_cache_key.as_str(), SCHEDULER_AFFINITY_TTL)
            .is_none());
    }

    #[tokio::test]
    async fn fallback_success_rewarms_scheduler_affinity_after_failed_candidate_invalidates() {
        let state = AppState::new().expect("gateway state should build");
        let failed_plan = sample_plan();
        let mut success_plan = sample_plan();
        success_plan.provider_id = "prov-2".to_string();
        success_plan.endpoint_id = "ep-2".to_string();
        success_plan.key_id = "key-2".to_string();
        let report_context = json!({
            "api_key_id": "api-key-1",
            "client_api_format": "openai:chat",
            "model": "gpt-5",
        });
        let cache_key =
            build_scheduler_affinity_cache_key_for_api_key_id("api-key-1", "openai:chat", "gpt-5")
                .expect("scheduler affinity cache key should build");

        state.remember_scheduler_affinity_target(
            &cache_key,
            SchedulerAffinityTarget {
                provider_id: "prov-1".to_string(),
                endpoint_id: "ep-1".to_string(),
                key_id: "key-1".to_string(),
            },
            SCHEDULER_AFFINITY_TTL,
            16,
        );
        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &failed_plan,
                report_context: Some(&report_context),
            },
            LocalExecutionEffect::AttemptFailure(LocalAttemptFailureEffect {
                status_code: 429,
                classification: LocalFailoverClassification::RetryUpstreamFailure,
            }),
        )
        .await;
        assert!(state
            .read_scheduler_affinity_target(cache_key.as_str(), SCHEDULER_AFFINITY_TTL)
            .is_none());

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &success_plan,
                report_context: Some(&report_context),
            },
            LocalExecutionEffect::HealthSuccess(LocalHealthSuccessEffect),
        )
        .await;

        assert_eq!(
            state.read_scheduler_affinity_target(cache_key.as_str(), SCHEDULER_AFFINITY_TTL),
            Some(SchedulerAffinityTarget {
                provider_id: "prov-2".to_string(),
                endpoint_id: "ep-2".to_string(),
                key_id: "key-2".to_string(),
            })
        );
    }

    #[test]
    fn configured_stop_pattern_does_not_penalize_pool_feedback() {
        assert!(!local_candidate_failure_should_record_pool_error(
            LocalFailoverClassification::StopErrorPattern,
            400,
        ));
        assert!(!local_candidate_failure_should_record_pool_error(
            LocalFailoverClassification::RetryUpstreamFailure,
            400,
        ));
        assert!(local_candidate_failure_should_record_pool_error(
            LocalFailoverClassification::RetryUpstreamFailure,
            429,
        ));
    }

    #[test]
    fn terminal_pool_account_errors_project_pool_hard_state() {
        assert_eq!(
            pool_score_hard_state_for_status(
                400,
                Some(r#"{"error":{"message":"deactivated_workspace"}}"#),
            ),
            Some(PoolMemberHardState::Banned)
        );
        assert_eq!(
            pool_score_hard_state_for_status(
                402,
                Some(r#"{"error":{"message":"payment required"}}"#),
            ),
            Some(PoolMemberHardState::QuotaExhausted)
        );
    }

    #[tokio::test]
    async fn pool_account_error_does_not_open_key_circuit() {
        let Some(redis) = start_managed_redis_or_skip().await else {
            return;
        };
        let state = codex_state_with_redis(redis.redis_url(), "orchestration_pool_circuit");
        let plan = sample_codex_plan();
        let legacy_circuit = json!({
            "openai:responses": {
                "open": true,
                "reason": "legacy"
            }
        });
        state
            .update_provider_catalog_key_health_state(
                &plan.key_id,
                true,
                None,
                Some(&legacy_circuit),
            )
            .await
            .expect("legacy circuit should seed");

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: None,
            },
            LocalExecutionEffect::PoolError(LocalPoolErrorEffect {
                status_code: 401,
                classification: LocalFailoverClassification::StopErrorPattern,
                headers: &BTreeMap::new(),
                error_body: Some(r#"{"error":{"message":"account has been deactivated"}}"#),
            }),
        )
        .await;

        let stored_key = state
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&plan.key_id))
            .await
            .expect("provider catalog keys should load")
            .into_iter()
            .next()
            .expect("stored key should exist");
        assert_eq!(stored_key.circuit_breaker_by_format, None);
    }

    #[tokio::test]
    async fn oauth_invalidation_marks_codex_key_invalid() {
        let state = codex_state();
        let plan = sample_codex_plan();

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: None,
            },
            LocalExecutionEffect::OauthInvalidation(LocalOAuthInvalidationEffect {
                status_code: 401,
                response_text: Some(
                    r#"{"error":{"message":"session expired","type":"invalid_request_error"}}"#,
                ),
            }),
        )
        .await;

        let stored_key = state
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&plan.key_id))
            .await
            .expect("provider catalog keys should load")
            .into_iter()
            .next()
            .expect("stored key should exist");
        assert!(stored_key.oauth_invalid_at_unix_secs.is_some());
        assert_eq!(
            stored_key.oauth_invalid_reason.as_deref(),
            Some("[OAUTH_EXPIRED] session expired")
        );
        assert_eq!(
            stored_key
                .status_snapshot
                .as_ref()
                .and_then(|value| value.get("oauth"))
                .and_then(|value| value.get("code"))
                .and_then(Value::as_str),
            Some("invalid")
        );
    }

    #[tokio::test]
    async fn oauth_invalidation_ignores_generic_codex_403() {
        let state = codex_state();
        let plan = sample_codex_plan();

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: None,
            },
            LocalExecutionEffect::OauthInvalidation(LocalOAuthInvalidationEffect {
                status_code: 403,
                response_text: Some(r#"{"error":{"message":"forbidden"}}"#),
            }),
        )
        .await;

        let stored_key = state
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&plan.key_id))
            .await
            .expect("provider catalog keys should load")
            .into_iter()
            .next()
            .expect("stored key should exist");
        assert_eq!(stored_key.oauth_invalid_at_unix_secs, None);
        assert_eq!(stored_key.oauth_invalid_reason, None);
    }

    #[tokio::test]
    async fn health_failure_projection_updates_key_health_for_format() {
        let state = health_state();
        let plan = sample_plan();

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: None,
            },
            LocalExecutionEffect::HealthFailure(LocalHealthFailureEffect {
                status_code: 503,
                classification: LocalFailoverClassification::RetryUpstreamFailure,
            }),
        )
        .await;

        let stored_key = state
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&plan.key_id))
            .await
            .expect("provider catalog keys should load")
            .into_iter()
            .next()
            .expect("stored key should exist");
        assert_eq!(
            stored_key.health_by_format,
            Some(json!({
                "openai:chat": {
                    "health_score": 0.6,
                    "consecutive_failures": 1,
                    "last_failure_at": stored_key
                        .health_by_format
                        .as_ref()
                        .and_then(|value| value.get("openai:chat"))
                        .and_then(|value| value.get("last_failure_at"))
                        .cloned()
                        .unwrap_or(Value::Null)
                }
            }))
        );
    }

    #[tokio::test]
    async fn health_failure_opens_circuit_after_eight_consecutive_failures() {
        let state = health_state();
        let plan = sample_plan();

        for _ in 0..8 {
            apply_local_execution_effect(
                &state,
                LocalExecutionEffectContext {
                    plan: &plan,
                    report_context: None,
                },
                LocalExecutionEffect::HealthFailure(LocalHealthFailureEffect {
                    status_code: 503,
                    classification: LocalFailoverClassification::RetryUpstreamFailure,
                }),
            )
            .await;
        }

        let stored_key = state
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&plan.key_id))
            .await
            .expect("provider catalog keys should load")
            .into_iter()
            .next()
            .expect("stored key should exist");
        let circuit = stored_key
            .circuit_breaker_by_format
            .as_ref()
            .and_then(|value| value.get("openai:chat"))
            .expect("format circuit should be stored");
        assert_eq!(circuit["open"], json!(true));
        assert_eq!(circuit["reason"], json!("consecutive_failures_8"));
        assert_eq!(circuit["probe_interval_minutes"], json!(1));
        assert!(circuit["next_probe_at_unix_secs"].as_u64().is_some());
        assert_eq!(
            circuit["request_results_window"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            8
        );
    }

    #[tokio::test]
    async fn pool_health_failure_does_not_open_key_circuit_after_eight_consecutive_failures() {
        let state = pool_health_state();
        let plan = sample_plan();

        for _ in 0..8 {
            apply_local_execution_effect(
                &state,
                LocalExecutionEffectContext {
                    plan: &plan,
                    report_context: None,
                },
                LocalExecutionEffect::HealthFailure(LocalHealthFailureEffect {
                    status_code: 503,
                    classification: LocalFailoverClassification::RetryUpstreamFailure,
                }),
            )
            .await;
        }

        let stored_key = state
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&plan.key_id))
            .await
            .expect("provider catalog keys should load")
            .into_iter()
            .next()
            .expect("stored key should exist");
        assert_eq!(stored_key.circuit_breaker_by_format, None);
        assert_eq!(
            stored_key
                .health_by_format
                .as_ref()
                .and_then(|value| value.get("openai:chat"))
                .and_then(|value| value.get("consecutive_failures"))
                .and_then(Value::as_u64),
            Some(8)
        );
    }

    #[tokio::test]
    async fn health_success_projection_resets_key_health_for_format() {
        let state = health_state();
        let plan = sample_plan();

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: None,
            },
            LocalExecutionEffect::HealthFailure(LocalHealthFailureEffect {
                status_code: 503,
                classification: LocalFailoverClassification::RetryUpstreamFailure,
            }),
        )
        .await;
        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: None,
            },
            LocalExecutionEffect::HealthSuccess(LocalHealthSuccessEffect),
        )
        .await;

        let stored_key = state
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&plan.key_id))
            .await
            .expect("provider catalog keys should load")
            .into_iter()
            .next()
            .expect("stored key should exist");
        assert_eq!(
            stored_key.health_by_format,
            Some(json!({
                "openai:chat": {
                    "health_score": 1.0,
                    "consecutive_failures": 0,
                    "last_failure_at": Value::Null
                }
            }))
        );
    }

    #[tokio::test]
    async fn health_success_projection_closes_key_circuit_for_format() {
        let mut key = sample_health_key();
        key.circuit_breaker_by_format = Some(json!({
            "openai:chat": {
                "open": true,
                "reason": "account_deactivated_401",
                "next_probe_at_unix_secs": 1_760_001_920u64
            }
        }));
        let state = health_state_with_key(key);
        let plan = sample_plan();

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: None,
            },
            LocalExecutionEffect::HealthSuccess(LocalHealthSuccessEffect),
        )
        .await;

        let stored_key = state
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&plan.key_id))
            .await
            .expect("provider catalog keys should load")
            .into_iter()
            .next()
            .expect("stored key should exist");
        let circuit = stored_key
            .circuit_breaker_by_format
            .as_ref()
            .and_then(|value| value.get("openai:chat"))
            .expect("format circuit should be stored");
        assert_eq!(circuit["open"], json!(false));
        assert_eq!(circuit["reason"], Value::Null);
        assert_eq!(circuit["next_probe_at_unix_secs"], Value::Null);
    }

    #[tokio::test]
    async fn adaptive_rate_limit_effect_updates_adaptive_key_observation() {
        let state = adaptive_state();
        let plan = sample_plan();
        let cache_key =
            build_scheduler_affinity_cache_key_for_api_key_id("api-key-1", "openai:chat", "gpt-5")
                .expect("scheduler affinity cache key should build");
        let target = SchedulerAffinityTarget {
            provider_id: plan.provider_id.clone(),
            endpoint_id: plan.endpoint_id.clone(),
            key_id: plan.key_id.clone(),
        };
        state.remember_scheduler_affinity_target(
            &cache_key,
            target.clone(),
            SCHEDULER_AFFINITY_TTL,
            16,
        );
        let initial_epoch = state.scheduler_affinity_epoch();

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: None,
            },
            LocalExecutionEffect::AdaptiveRateLimit(LocalAdaptiveRateLimitEffect {
                status_code: 429,
                classification: LocalFailoverClassification::RetryUpstreamFailure,
                headers: Some(&BTreeMap::from([(
                    "x-ratelimit-limit-requests".to_string(),
                    "42".to_string(),
                )])),
            }),
        )
        .await;

        let stored_key = state
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&plan.key_id))
            .await
            .expect("provider catalog keys should load")
            .into_iter()
            .next()
            .expect("stored key should exist");
        assert_eq!(stored_key.rpm_429_count, Some(2));
        assert_eq!(stored_key.last_429_type.as_deref(), Some("rpm"));
        assert!(stored_key.last_429_at_unix_secs.is_some());
        assert_eq!(
            stored_key
                .status_snapshot
                .as_ref()
                .and_then(|value| value.get("observation_count")),
            Some(&json!(1))
        );
        assert_eq!(
            stored_key
                .status_snapshot
                .as_ref()
                .and_then(|value| value.get("header_observation_count")),
            Some(&json!(1))
        );
        assert_eq!(
            stored_key
                .status_snapshot
                .as_ref()
                .and_then(|value| value.get("latest_upstream_limit")),
            Some(&json!(42))
        );
        assert_eq!(
            stored_key
                .status_snapshot
                .as_ref()
                .and_then(|value| value.get("learning_confidence")),
            Some(&json!(0.3))
        );
        assert_eq!(
            stored_key
                .status_snapshot
                .as_ref()
                .and_then(|value| value.get("enforcement_active")),
            Some(&json!(false))
        );
        assert_eq!(state.scheduler_affinity_epoch(), initial_epoch);
        assert_eq!(
            state.read_scheduler_affinity_target(cache_key.as_str(), SCHEDULER_AFFINITY_TTL),
            Some(target)
        );
    }

    #[tokio::test]
    async fn adaptive_rate_limit_effect_ignores_fixed_limit_key() {
        let state = fixed_limit_state();
        let plan = sample_plan();

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: None,
            },
            LocalExecutionEffect::AdaptiveRateLimit(LocalAdaptiveRateLimitEffect {
                status_code: 429,
                classification: LocalFailoverClassification::RetryUpstreamFailure,
                headers: Some(&BTreeMap::from([(
                    "x-ratelimit-limit-requests".to_string(),
                    "42".to_string(),
                )])),
            }),
        )
        .await;

        let stored_key = state
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&plan.key_id))
            .await
            .expect("provider catalog keys should load")
            .into_iter()
            .next()
            .expect("stored key should exist");
        assert_eq!(stored_key.rpm_429_count, None);
        assert_eq!(stored_key.last_429_at_unix_secs, None);
        assert_eq!(stored_key.last_429_type, None);
    }

    #[tokio::test]
    async fn adaptive_rate_limit_effect_records_429_as_rpm_observation() {
        let mut key = sample_health_key();
        key.rpm_limit = None;
        key.learned_rpm_limit = Some(20);
        let state = adaptive_state_with_request_candidates(key, Vec::new());
        let plan = sample_plan();

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: None,
            },
            LocalExecutionEffect::AdaptiveRateLimit(LocalAdaptiveRateLimitEffect {
                status_code: 429,
                classification: LocalFailoverClassification::RetryStatusCode,
                headers: None,
            }),
        )
        .await;

        let stored_key = state
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&plan.key_id))
            .await
            .expect("provider catalog keys should load")
            .into_iter()
            .next()
            .expect("stored key should exist");
        assert_eq!(stored_key.rpm_429_count, Some(1));
        assert_eq!(stored_key.learned_rpm_limit, Some(20));
        assert_eq!(stored_key.last_429_type.as_deref(), Some("rpm"));
    }

    #[tokio::test]
    async fn adaptive_success_effect_expands_limit_from_recent_rpm_usage() {
        let now_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;
        let mut key = sample_adaptive_key();
        key.learned_rpm_limit = Some(20);
        key.last_rpm_peak = Some(25);
        key.last_429_at_unix_secs = Some(now_unix_secs.saturating_sub(600));
        key.adjustment_history = Some(json!([
            {
                "timestamp": "2026-04-19T00:00:00Z",
                "old_limit": 0,
                "new_limit": 20,
                "reason": "rpm_429",
                "confidence": 0.8
            }
        ]));
        key.utilization_samples = Some(json!([
            {"ts": now_unix_secs.saturating_sub(40), "util": 0.90},
            {"ts": now_unix_secs.saturating_sub(30), "util": 0.95},
            {"ts": now_unix_secs.saturating_sub(20), "util": 0.85},
            {"ts": now_unix_secs.saturating_sub(10), "util": 0.80}
        ]));
        let state = adaptive_state_with_request_candidates(
            key,
            vec![StoredRequestCandidate::new(
                "candidate-1".to_string(),
                "req-1".to_string(),
                None,
                None,
                None,
                None,
                0,
                0,
                Some("prov-1".to_string()),
                Some("ep-1".to_string()),
                Some("key-1".to_string()),
                RequestCandidateStatus::Success,
                None,
                false,
                Some(200),
                None,
                None,
                Some(10),
                Some(19),
                None,
                None,
                i64::try_from(now_unix_secs.saturating_sub(30) * 1000)
                    .expect("candidate created_at should fit i64"),
                Some(
                    i64::try_from(now_unix_secs.saturating_sub(30) * 1000)
                        .expect("candidate started_at should fit i64"),
                ),
                Some(
                    i64::try_from(now_unix_secs.saturating_sub(29) * 1000)
                        .expect("candidate finished_at should fit i64"),
                ),
            )
            .expect("request candidate should build")],
        );
        let plan = sample_plan();
        let cache_key =
            build_scheduler_affinity_cache_key_for_api_key_id("api-key-1", "openai:chat", "gpt-5")
                .expect("scheduler affinity cache key should build");
        let target = SchedulerAffinityTarget {
            provider_id: plan.provider_id.clone(),
            endpoint_id: plan.endpoint_id.clone(),
            key_id: plan.key_id.clone(),
        };
        state.remember_scheduler_affinity_target(
            &cache_key,
            target.clone(),
            SCHEDULER_AFFINITY_TTL,
            16,
        );
        let initial_epoch = state.scheduler_affinity_epoch();

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: None,
            },
            LocalExecutionEffect::AdaptiveSuccess(LocalAdaptiveSuccessEffect),
        )
        .await;

        let stored_key = state
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&plan.key_id))
            .await
            .expect("provider catalog keys should load")
            .into_iter()
            .next()
            .expect("stored key should exist");
        assert_eq!(stored_key.learned_rpm_limit, Some(25));
        assert_eq!(stored_key.utilization_samples, Some(json!([])));
        assert_eq!(
            stored_key
                .adjustment_history
                .as_ref()
                .and_then(Value::as_array)
                .and_then(|items| items.last())
                .and_then(Value::as_object)
                .and_then(|record| record.get("reason"))
                .and_then(Value::as_str),
            Some("high_utilization")
        );
        assert_eq!(state.scheduler_affinity_epoch(), initial_epoch);
        assert_eq!(
            state.read_scheduler_affinity_target(cache_key.as_str(), SCHEDULER_AFFINITY_TTL),
            Some(target)
        );
    }
}
