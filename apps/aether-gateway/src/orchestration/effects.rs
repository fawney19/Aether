use std::collections::BTreeMap;

use aether_admin::provider::quota as admin_provider_quota_pure;
use aether_contracts::{ExecutionPlan, ExecutionTelemetry};
use aether_scheduler_core::{
    build_scheduler_affinity_cache_key_for_api_key_id, count_recent_rpm_requests_for_provider_key,
    SchedulerAffinityTarget,
};
use aether_usage_runtime::{
    build_stream_terminal_usage_outcome, build_sync_terminal_usage_outcome,
    GatewayStreamReportRequest, GatewaySyncReportRequest, TerminalUsageOutcome,
};
use serde_json::Value;
use tracing::warn;

use super::{
    local_failover_error_message, project_local_adaptive_rate_limit,
    project_local_adaptive_success, project_local_failure_health, project_local_success_health,
    LocalFailoverClassification,
};
use crate::ai_serving::extract_pool_sticky_session_token;
use crate::clock::current_unix_secs;
use crate::handlers::shared::provider_pool::admin_provider_pool_config_from_config_value;
use crate::handlers::shared::provider_pool::{
    record_admin_provider_pool_error, record_admin_provider_pool_stream_timeout,
    record_admin_provider_pool_success, AdminProviderPoolConfig,
};
use crate::scheduler::affinity::SCHEDULER_AFFINITY_TTL;
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
    runner: aether_data::redis::RedisKvRunner,
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
        }
        LocalExecutionEffect::PoolSuccessStream { payload } => {
            record_stream_pool_success_effect(state, context, payload).await;
        }
        LocalExecutionEffect::PoolError(effect) => {
            record_pool_error_effect(state, context, effect).await;
        }
        LocalExecutionEffect::PoolStreamTimeout => {
            record_pool_stream_timeout_effect(state, context).await;
        }
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
    build_scheduler_affinity_cache_key_for_api_key_id(
        report_context_string_field(report_context, "api_key_id")?,
        report_context_string_field(report_context, "client_api_format")?,
        report_context_string_field(report_context, "model")?,
    )
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

fn remember_successful_local_scheduler_affinity(
    state: &AppState,
    context: LocalExecutionEffectContext<'_>,
) {
    let Some(cache_key) = local_scheduler_affinity_cache_key(context.report_context) else {
        return;
    };
    let Some(target) = local_scheduler_affinity_target(context.plan) else {
        return;
    };

    state.remember_scheduler_affinity_target(
        &cache_key,
        target,
        SCHEDULER_AFFINITY_TTL,
        LOCAL_EXECUTION_SCHEDULER_AFFINITY_MAX_ENTRIES,
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
    let Some(runner) = state.redis_kv_runner() else {
        return None;
    };

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
        runner,
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
        let _ = state.remove_scheduler_affinity_cache_entry(&cache_key);
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
        &pool_context.runner,
        &context.plan.provider_id,
        &context.plan.key_id,
        &pool_context.pool_config,
        pool_context.sticky_session_token.as_deref(),
        total_tokens_used(&usage_outcome),
        resolve_ttfb_ms(payload.telemetry.as_ref()),
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

    if let Err(err) = state.update_provider_catalog_key(&updated_key).await {
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

    if let Err(err) = state.update_provider_catalog_key(&updated_key).await {
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
    let Some(health_by_format) = project_local_failure_health(
        current_key.health_by_format.as_ref(),
        api_format,
        effect.classification,
        effect.status_code,
        current_unix_secs(),
    ) else {
        return;
    };

    if let Err(err) = state
        .update_provider_catalog_key_format_health(
            &context.plan.key_id,
            api_format,
            &health_by_format,
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
    remember_successful_local_scheduler_affinity(state, context);

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
    let Some(health_by_format) =
        project_local_success_health(current_key.health_by_format.as_ref(), api_format)
    else {
        return;
    };

    if let Err(err) = state
        .update_provider_catalog_key_format_health(
            &context.plan.key_id,
            api_format,
            &health_by_format,
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
        &pool_context.runner,
        &context.plan.provider_id,
        &context.plan.key_id,
        &pool_context.pool_config,
        pool_context.sticky_session_token.as_deref(),
        total_tokens_used(&usage_outcome),
        resolve_ttfb_ms(payload.telemetry.as_ref()),
    )
    .await;
}

async fn record_pool_error_effect(
    state: &AppState,
    context: LocalExecutionEffectContext<'_>,
    effect: LocalPoolErrorEffect<'_>,
) {
    if !local_candidate_failure_should_record_pool_error(effect.classification, effect.status_code)
    {
        return;
    }

    let Some(pool_context) = resolve_pool_feedback_context(state, context).await else {
        return;
    };

    record_admin_provider_pool_error(
        &pool_context.runner,
        &context.plan.provider_id,
        &context.plan.key_id,
        &pool_context.pool_config,
        effect.status_code,
        effect.error_body,
        Some(effect.headers),
    )
    .await;
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
        LocalFailoverClassification::StopErrorPattern => false,
    }
}

fn local_candidate_failure_should_record_pool_error(
    classification: LocalFailoverClassification,
    status_code: u16,
) -> bool {
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
        &pool_context.runner,
        &context.plan.provider_id,
        &context.plan.key_id,
        &pool_context.pool_config,
    )
    .await;
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
    use aether_data_contracts::repository::provider_catalog::{
        StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
    };
    use serde_json::{json, Value};

    use super::{
        apply_local_execution_effect, local_candidate_failure_should_record_pool_error,
        LocalAdaptiveRateLimitEffect, LocalAdaptiveSuccessEffect, LocalAttemptFailureEffect,
        LocalExecutionEffect, LocalExecutionEffectContext, LocalHealthFailureEffect,
        LocalHealthSuccessEffect, LocalOAuthInvalidationEffect,
    };
    use crate::data::GatewayDataState;
    use crate::orchestration::LocalFailoverClassification;
    use crate::scheduler::affinity::SCHEDULER_AFFINITY_TTL;
    use crate::AppState;
    use aether_scheduler_core::{
        build_scheduler_affinity_cache_key_for_api_key_id, SchedulerAffinityTarget,
    };

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
            tls_profile: None,
            timeouts: None,
        }
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
            tls_profile: None,
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
            None,
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

    fn sample_health_provider() -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            "prov-1".to_string(),
            "openai".to_string(),
            Some("https://example.com".to_string()),
            "custom".to_string(),
        )
        .expect("provider should build")
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

        state.scheduler_affinity_cache.insert(
            cache_key.clone(),
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

        state.scheduler_affinity_cache.insert(
            cache_key.clone(),
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

        state.scheduler_affinity_cache.insert(
            cache_key.clone(),
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

        state.scheduler_affinity_cache.insert(
            cache_key.clone(),
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
        assert!(local_candidate_failure_should_record_pool_error(
            LocalFailoverClassification::RetryUpstreamFailure,
            429,
        ));
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
    async fn adaptive_rate_limit_effect_updates_adaptive_key_observation() {
        let state = adaptive_state();
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
    }
}
