use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex as StdMutex, Weak};
use std::time::Duration;

use aether_admin::provider::quota as admin_provider_quota_pure;
use aether_cache::ExpiringMap;
use aether_contracts::{ExecutionPlan, ExecutionTelemetry};
use aether_data_contracts::repository::pool_scores::{
    PoolMemberHardState, PoolMemberIdentity, PoolMemberScheduleFeedback,
};
use aether_data_contracts::repository::provider_catalog::{
    ProviderCatalogKeyAdaptiveState, ProviderCatalogKeyAdaptiveStateUpdate,
    ProviderCatalogKeyHealthStateUpdate,
};
use aether_routing_core::RoutingPoolPolicyOverride;
use aether_scheduler_core::{
    build_scheduler_affinity_cache_key_for_api_key_id_with_client_session_and_scope,
    count_recent_rpm_requests_for_provider_key, ClientSessionAffinity, SchedulerAffinityTarget,
};
use aether_usage_runtime::{
    build_stream_terminal_usage_outcome, build_sync_terminal_usage_outcome,
    GatewayStreamReportRequest, GatewaySyncReportRequest, TerminalUsageOutcome,
};
use serde_json::Value;
use tokio::sync::Mutex as TokioMutex;
use tracing::warn;

use super::{
    classify_failure_disposition, local_failover_error_message, project_local_adaptive_rate_limit,
    project_local_adaptive_success, project_local_failure_health, project_local_key_circuit_closed,
    project_local_key_circuit_failure, project_local_success_health,
    resolve_local_failover_analysis_for_attempt, FailureScope, LocalFailoverAnalysis,
    LocalFailoverClassification,
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
    AdminProviderPoolSchedulingPreset,
};
use crate::orchestration::{
    local_execution_candidate_metadata_from_report_context,
    ROUTING_POOL_POLICY_OVERRIDE_REPORT_FIELD,
};
use crate::scheduler::affinity::{
    scheduler_affinity_policy_context_from_report_context, SCHEDULER_AFFINITY_POLICY_REPORT_FIELD,
    SCHEDULER_AFFINITY_TTL,
};
use crate::AppState;

const POOL_SCORE_FEEDBACK_GATE_MAX_ENTRIES: usize = 50_000;
const HEALTH_SUCCESS_PERSIST_GATE_MAX_ENTRIES: usize = 50_000;
const ADAPTIVE_SUCCESS_PERSIST_GATE_MAX_ENTRIES: usize = 50_000;
const POOL_SCORE_SUCCESS_FEEDBACK_MIN_INTERVAL_ENV: &str =
    "AETHER_GATEWAY_POOL_SCORE_SUCCESS_FEEDBACK_MIN_INTERVAL_SECS";
const POOL_SCORE_FAILURE_FEEDBACK_MIN_INTERVAL_ENV: &str =
    "AETHER_GATEWAY_POOL_SCORE_FAILURE_FEEDBACK_MIN_INTERVAL_SECS";
const HEALTH_SUCCESS_PERSIST_MIN_INTERVAL_ENV: &str =
    "AETHER_GATEWAY_PROVIDER_KEY_HEALTH_SUCCESS_PERSIST_MIN_INTERVAL_SECS";
const ADAPTIVE_SUCCESS_PERSIST_MIN_INTERVAL_ENV: &str =
    "AETHER_GATEWAY_PROVIDER_KEY_ADAPTIVE_SUCCESS_PERSIST_MIN_INTERVAL_SECS";
const DEFAULT_POOL_SCORE_SUCCESS_FEEDBACK_MIN_INTERVAL_SECS: u64 = 5;
const DEFAULT_POOL_SCORE_FAILURE_FEEDBACK_MIN_INTERVAL_SECS: u64 = 1;
const DEFAULT_HEALTH_SUCCESS_PERSIST_MIN_INTERVAL_SECS: u64 = 5;
const DEFAULT_ADAPTIVE_SUCCESS_PERSIST_MIN_INTERVAL_SECS: u64 = 5;
const MAX_POOL_SCORE_FEEDBACK_MIN_INTERVAL_SECS: u64 = 300;
const PROVIDER_KEY_EFFECT_LOCK_PRUNE_THRESHOLD: usize = 8_192;
// Same-process writers are serialized by the per-key lock. Keep remote-writer
// retries bounded so request/report completion cannot accumulate a long DB tail.
const PROVIDER_KEY_STATE_CAS_MAX_ATTEMPTS: usize = 4;

#[derive(Debug)]
struct ProviderKeyEffectLockPoolState {
    entries: HashMap<String, Weak<TokioMutex<()>>>,
    accesses_since_prune: usize,
    next_growth_prune_at: usize,
    #[cfg(test)]
    prune_count: usize,
}

impl ProviderKeyEffectLockPoolState {
    fn new(min_prune_threshold: usize) -> Self {
        Self {
            entries: HashMap::new(),
            accesses_since_prune: 0,
            next_growth_prune_at: min_prune_threshold,
            #[cfg(test)]
            prune_count: 0,
        }
    }
}

#[derive(Debug)]
struct ProviderKeyEffectLockPool {
    state: StdMutex<ProviderKeyEffectLockPoolState>,
    min_prune_threshold: usize,
}

impl Default for ProviderKeyEffectLockPool {
    fn default() -> Self {
        Self::new(PROVIDER_KEY_EFFECT_LOCK_PRUNE_THRESHOLD)
    }
}

impl ProviderKeyEffectLockPool {
    fn new(min_prune_threshold: usize) -> Self {
        let min_prune_threshold = min_prune_threshold.max(1);
        Self {
            state: StdMutex::new(ProviderKeyEffectLockPoolState::new(min_prune_threshold)),
            min_prune_threshold,
        }
    }

    fn lock_for(&self, key_id: &str) -> Arc<TokioMutex<()>> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.accesses_since_prune = state.accesses_since_prune.saturating_add(1);
        let entry_count = state.entries.len();
        let growth_prune_due = entry_count >= state.next_growth_prune_at;
        let maintenance_prune_due = entry_count >= self.min_prune_threshold
            && state.accesses_since_prune >= entry_count.max(self.min_prune_threshold);
        if growth_prune_due || maintenance_prune_due {
            self.prune_inactive_locks(&mut state);
        }

        if let Some(existing) = state.entries.get(key_id).and_then(Weak::upgrade) {
            return existing;
        }
        let lock = Arc::new(TokioMutex::new(()));
        state
            .entries
            .insert(key_id.to_string(), Arc::downgrade(&lock));
        lock
    }

    fn prune_inactive_locks(&self, state: &mut ProviderKeyEffectLockPoolState) {
        state.entries.retain(|_, lock| lock.strong_count() > 0);
        let active_entries = state.entries.len();
        state.next_growth_prune_at = if active_entries < self.min_prune_threshold {
            self.min_prune_threshold
        } else {
            active_entries.saturating_mul(2)
        };
        state.accesses_since_prune = 0;
        #[cfg(test)]
        {
            state.prune_count = state.prune_count.saturating_add(1);
        }
    }
}

static POOL_SCORE_FEEDBACK_GATE: LazyLock<ExpiringMap<String, ()>> =
    LazyLock::new(ExpiringMap::new);
static HEALTH_SUCCESS_PERSIST_GATE: LazyLock<ExpiringMap<String, ()>> =
    LazyLock::new(ExpiringMap::new);
static ADAPTIVE_SUCCESS_PERSIST_GATE: LazyLock<ExpiringMap<String, u64>> =
    LazyLock::new(ExpiringMap::new);
static ADAPTIVE_SUCCESS_PERSIST_GATE_NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);
static PROVIDER_KEY_EFFECT_LOCKS: LazyLock<ProviderKeyEffectLockPool> =
    LazyLock::new(ProviderKeyEffectLockPool::default);
static POOL_SCORE_SUCCESS_FEEDBACK_MIN_INTERVAL: LazyLock<Duration> = LazyLock::new(|| {
    pool_score_feedback_interval_from_env(
        POOL_SCORE_SUCCESS_FEEDBACK_MIN_INTERVAL_ENV,
        DEFAULT_POOL_SCORE_SUCCESS_FEEDBACK_MIN_INTERVAL_SECS,
    )
});
static POOL_SCORE_FAILURE_FEEDBACK_MIN_INTERVAL: LazyLock<Duration> = LazyLock::new(|| {
    pool_score_feedback_interval_from_env(
        POOL_SCORE_FAILURE_FEEDBACK_MIN_INTERVAL_ENV,
        DEFAULT_POOL_SCORE_FAILURE_FEEDBACK_MIN_INTERVAL_SECS,
    )
});
static HEALTH_SUCCESS_PERSIST_MIN_INTERVAL: LazyLock<Duration> = LazyLock::new(|| {
    pool_score_feedback_interval_from_env(
        HEALTH_SUCCESS_PERSIST_MIN_INTERVAL_ENV,
        DEFAULT_HEALTH_SUCCESS_PERSIST_MIN_INTERVAL_SECS,
    )
});
static ADAPTIVE_SUCCESS_PERSIST_MIN_INTERVAL: LazyLock<Duration> = LazyLock::new(|| {
    pool_score_feedback_interval_from_env(
        ADAPTIVE_SUCCESS_PERSIST_MIN_INTERVAL_ENV,
        DEFAULT_ADAPTIVE_SUCCESS_PERSIST_MIN_INTERVAL_SECS,
    )
});

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
pub(crate) struct LocalOAuthSuccessEffect<'a> {
    pub(crate) status_code: u16,
    pub(crate) request_started_at_unix_ms: Option<u64>,
    pub(crate) request_order_id: Option<&'a str>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum LocalExecutionEffect<'a> {
    AttemptFailure(LocalAttemptFailureEffect),
    AdaptiveRateLimit(LocalAdaptiveRateLimitEffect<'a>),
    HealthFailure(LocalHealthFailureEffect),
    HealthSuccess(LocalHealthSuccessEffect),
    AdaptiveSuccess(LocalAdaptiveSuccessEffect),
    OauthInvalidation(LocalOAuthInvalidationEffect<'a>),
    OauthSuccess(LocalOAuthSuccessEffect<'a>),
    PoolSuccessSync {
        payload: &'a GatewaySyncReportRequest,
    },
    PoolSuccessStream {
        payload: &'a GatewayStreamReportRequest,
    },
    PoolError(LocalPoolErrorEffect<'a>),
    PoolStreamTimeout,
}

#[derive(Debug)]
struct OwnedLocalOAuthSuccessEffect {
    status_code: u16,
    provider_id: String,
    endpoint_id: String,
    key_id: String,
    authorization: String,
    request_started_at_unix_ms: u64,
    request_order_id: String,
    observed_credential_generation: Option<String>,
}

fn owned_local_oauth_success_effect(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    effect: LocalOAuthSuccessEffect<'_>,
) -> Option<OwnedLocalOAuthSuccessEffect> {
    if !(200..300).contains(&effect.status_code) {
        return None;
    }
    let request_started_at_unix_ms = effect.request_started_at_unix_ms?;
    let request_order_id = effect
        .request_order_id
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let authorization = execution_plan_authorization(plan)?.trim().to_string();
    if authorization.is_empty() || bearer_access_token(&authorization).is_none() {
        return None;
    }
    Some(OwnedLocalOAuthSuccessEffect {
        status_code: effect.status_code,
        provider_id: plan.provider_id.clone(),
        endpoint_id: plan.endpoint_id.clone(),
        key_id: plan.key_id.clone(),
        authorization,
        request_started_at_unix_ms,
        request_order_id: request_order_id.to_string(),
        observed_credential_generation: report_context_string_field(
            report_context,
            "codex_credential_generation",
        )
        .map(ToOwned::to_owned),
    })
}

/// Schedule a fenced Codex OAuth-success observation after provider headers are available.
/// Only the small identity/observation tuple is moved into the task; request bodies and plans
/// remain owned by the caller.
pub(crate) fn spawn_local_oauth_success_effect(
    state: AppState,
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    effect: LocalOAuthSuccessEffect<'_>,
) {
    let Some(effect) = owned_local_oauth_success_effect(plan, report_context, effect) else {
        return;
    };
    tokio::spawn(async move {
        record_oauth_success_effect_owned(&state, effect).await;
    });
}

/// Inputs for the terminal effects of a failed streaming attempt.
///
/// The status/body are deliberately supplied by the transport-specific caller:
/// a WebSocket terminal event may carry its own status and error body, while a
/// normal stream failure gets them from the HTTP response. Keeping this type at
/// the orchestration boundary prevents each transport from rebuilding the
/// health, adaptive, OAuth, and pool effect sequence independently.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LocalStreamFailureEffect<'a> {
    pub(crate) status_code: u16,
    pub(crate) headers: &'a BTreeMap<String, String>,
    pub(crate) response_text: Option<&'a str>,
    pub(crate) stream_timeout: bool,
}

impl<'a> LocalStreamFailureEffect<'a> {
    pub(crate) const fn new(
        status_code: u16,
        headers: &'a BTreeMap<String, String>,
        response_text: Option<&'a str>,
    ) -> Self {
        Self {
            status_code,
            headers,
            response_text,
            stream_timeout: false,
        }
    }

    pub(crate) const fn with_stream_timeout(mut self) -> Self {
        self.stream_timeout = true;
        self
    }
}

struct PoolFeedbackContext {
    pool_config: AdminProviderPoolConfig,
    sticky_session_token: Option<String>,
}

#[derive(Debug, Clone)]
enum LocalExecutionAuthConfigFence {
    Unfenced,
    Fenced(String),
}

impl LocalExecutionAuthConfigFence {
    fn encrypted_auth_config(&self) -> Option<&str> {
        match self {
            Self::Unfenced => None,
            Self::Fenced(ciphertext) => Some(ciphertext),
        }
    }
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
        LocalExecutionEffect::OauthSuccess(effect) => {
            record_oauth_success_effect(state, context, effect).await;
        }
        LocalExecutionEffect::PoolSuccessSync { payload } => {
            record_sync_pool_success_effect(state, context, payload).await;
            release_local_pool_key_lease(state, context).await;
        }
        LocalExecutionEffect::PoolSuccessStream { payload } => {
            record_stream_pool_success_effect(state, context, payload).await;
            release_local_pool_key_lease(state, context).await;
        }
        LocalExecutionEffect::PoolError(effect) => {
            record_pool_error_effect(state, context, effect).await;
            release_local_pool_key_lease(state, context).await;
        }
        LocalExecutionEffect::PoolStreamTimeout => {
            record_pool_stream_timeout_effect(state, context).await;
            release_local_pool_key_lease(state, context).await;
        }
    }
}

/// Apply the provider/key effects shared by every successful streaming
/// transport. Usage persistence and request-candidate terminal status remain
/// owned by the report layer; this helper only projects execution health,
/// adaptive state, and pool feedback.
pub(crate) async fn apply_local_stream_success_effects(
    state: &AppState,
    context: LocalExecutionEffectContext<'_>,
    payload: &GatewayStreamReportRequest,
) {
    apply_local_execution_effect(
        state,
        context,
        LocalExecutionEffect::HealthSuccess(LocalHealthSuccessEffect),
    )
    .await;
    apply_local_execution_effect(
        state,
        context,
        LocalExecutionEffect::AdaptiveSuccess(LocalAdaptiveSuccessEffect),
    )
    .await;
    apply_local_execution_effect(
        state,
        context,
        LocalExecutionEffect::PoolSuccessStream { payload },
    )
    .await;
}

/// Apply the provider/key effects shared by every failed streaming attempt.
/// The returned analysis is the same failover classification used by the
/// normal stream runtime, allowing the caller to make a transport-specific
/// retry/close decision without re-running policy evaluation.
pub(crate) async fn apply_local_stream_failure_effects(
    state: &AppState,
    context: LocalExecutionEffectContext<'_>,
    effect: LocalStreamFailureEffect<'_>,
) -> LocalFailoverAnalysis {
    let analysis = resolve_local_failover_analysis_for_attempt(
        state,
        context.plan,
        context.report_context,
        effect.status_code,
        effect.response_text,
    )
    .await;

    if effect.stream_timeout {
        apply_local_execution_effect(state, context, LocalExecutionEffect::PoolStreamTimeout).await;
    }
    apply_local_execution_effect(
        state,
        context,
        LocalExecutionEffect::AttemptFailure(LocalAttemptFailureEffect {
            status_code: effect.status_code,
            classification: analysis.classification,
        }),
    )
    .await;
    apply_local_execution_effect(
        state,
        context,
        LocalExecutionEffect::AdaptiveRateLimit(LocalAdaptiveRateLimitEffect {
            status_code: effect.status_code,
            classification: analysis.classification,
            headers: Some(effect.headers),
        }),
    )
    .await;
    apply_local_execution_effect(
        state,
        context,
        LocalExecutionEffect::HealthFailure(LocalHealthFailureEffect {
            status_code: effect.status_code,
            classification: analysis.classification,
        }),
    )
    .await;
    apply_local_execution_effect(
        state,
        context,
        LocalExecutionEffect::OauthInvalidation(LocalOAuthInvalidationEffect {
            status_code: effect.status_code,
            response_text: effect.response_text,
        }),
    )
    .await;
    apply_local_execution_effect(
        state,
        context,
        LocalExecutionEffect::PoolError(LocalPoolErrorEffect {
            status_code: effect.status_code,
            classification: analysis.classification,
            headers: effect.headers,
            error_body: effect.response_text,
        }),
    )
    .await;

    analysis
}

pub(crate) async fn release_local_pool_key_lease(
    state: &AppState,
    context: LocalExecutionEffectContext<'_>,
) {
    let metadata = local_execution_candidate_metadata_from_report_context(context.report_context);
    let Some(lease) = metadata.pool_key_lease else {
        return;
    };
    release_pool_key_lease(state, &lease).await;
}

/// Releases a lease carried by a planned-but-not-started report context. This
/// path has no execution plan yet, so it intentionally omits candidate health
/// logging and only performs the distributed lock cleanup.
pub(crate) async fn release_pool_key_lease_from_report_context(
    state: &AppState,
    report_context: Option<&Value>,
) {
    let metadata = local_execution_candidate_metadata_from_report_context(report_context);
    let Some(lease) = metadata.pool_key_lease else {
        return;
    };
    release_pool_key_lease(state, &lease).await;
}

async fn release_pool_key_lease(state: &AppState, lease: &aether_runtime_state::RuntimeLockLease) {
    if let Err(err) =
        release_admin_provider_pool_key_lease(state.runtime_state.as_ref(), lease).await
    {
        warn!(
            error = ?err,
            "gateway orchestration effects: failed to release a planned pool key lease"
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

fn report_context_u64_field(report_context: Option<&Value>, field: &str) -> Option<u64> {
    report_context
        .and_then(|context| context.get(field))
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|value| value.parse::<u64>().ok()))
        })
}

fn local_scheduler_affinity_cache_key(report_context: Option<&Value>) -> Option<String> {
    let client_session_affinity = local_client_session_affinity(report_context);
    let policy_context = scheduler_affinity_policy_context_from_report_context(report_context);
    if report_context
        .and_then(|context| context.get(SCHEDULER_AFFINITY_POLICY_REPORT_FIELD))
        .is_some()
        && policy_context.is_none()
    {
        return None;
    }
    build_scheduler_affinity_cache_key_for_api_key_id_with_client_session_and_scope(
        report_context_string_field(report_context, "api_key_id")?,
        report_context_string_field(report_context, "client_api_format")?,
        report_context_string_field(report_context, "model")?,
        client_session_affinity.as_ref(),
        policy_context
            .as_ref()
            .and_then(|context| context.scope.as_ref()),
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

    crate::client_session_affinity::client_session_affinity_from_api_request(
        report_context_string_field(Some(report_context), "client_api_format").unwrap_or_default(),
        &headers,
        body_json,
    )
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

async fn capture_local_execution_auth_config_fence(
    state: &AppState,
    plan: &ExecutionPlan,
) -> Option<LocalExecutionAuthConfigFence> {
    let transport = match state
        .read_provider_transport_snapshot(&plan.provider_id, &plan.endpoint_id, &plan.key_id)
        .await
    {
        Ok(Some(transport)) => transport,
        Ok(None) => return None,
        Err(err) => {
            warn!(
                provider_id = %plan.provider_id,
                endpoint_id = %plan.endpoint_id,
                key_id = %plan.key_id,
                error = ?err,
                "gateway orchestration effects: failed to read transport for credential fencing"
            );
            return None;
        }
    };
    if !transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("codex")
        || !transport.key.auth_type.trim().eq_ignore_ascii_case("oauth")
    {
        return Some(LocalExecutionAuthConfigFence::Unfenced);
    }

    let authorization = execution_plan_authorization(plan)?;
    let current_uses_agent_identity =
        crate::provider_transport::is_codex_agent_identity_transport(&transport);
    let authorization_matches = if current_uses_agent_identity {
        crate::provider_transport::codex_agent_identity_authorization_matches_transport(
            &transport,
            authorization,
        )
    } else if crate::provider_transport::is_codex_agent_identity_authorization(authorization) {
        false
    } else {
        execution_plan_bearer_matches_transport(plan, &transport)
    };
    if !authorization_matches {
        return None;
    }

    match state
        .capture_provider_transport_auth_config_fence(&transport)
        .await
    {
        Ok(Some(ciphertext)) => Some(LocalExecutionAuthConfigFence::Fenced(ciphertext)),
        Ok(None) => None,
        Err(err) => {
            warn!(
                provider_id = %plan.provider_id,
                endpoint_id = %plan.endpoint_id,
                key_id = %plan.key_id,
                error = ?err,
                "gateway orchestration effects: failed to capture credential fence"
            );
            None
        }
    }
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

fn scheduler_cache_affinity_enabled(report_context: Option<&Value>) -> bool {
    report_context
        .and_then(|context| context.get(SCHEDULER_AFFINITY_POLICY_REPORT_FIELD))
        .is_some()
        && scheduler_affinity_policy_context_from_report_context(report_context)
            .is_some_and(|context| context.cache_affinity_enabled())
}

async fn remember_successful_local_scheduler_affinity(
    state: &AppState,
    context: LocalExecutionEffectContext<'_>,
) {
    if !scheduler_cache_affinity_enabled(context.report_context) {
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
    capture_local_execution_auth_config_fence(state, plan).await?;
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

    let Some(mut pool_config) =
        admin_provider_pool_config_from_config_value(transport.provider.config.as_ref())
    else {
        return None;
    };

    if let Some(override_policy) = context
        .report_context
        .and_then(|report_context| report_context.get(ROUTING_POOL_POLICY_OVERRIDE_REPORT_FIELD))
        .and_then(|value| serde_json::from_value::<RoutingPoolPolicyOverride>(value.clone()).ok())
        .filter(|override_policy| !override_policy.scheduling_presets.is_empty())
    {
        let scheduling_presets = override_policy
            .scheduling_presets
            .into_iter()
            .map(|preset| AdminProviderPoolSchedulingPreset {
                preset: preset.preset,
                enabled: preset.enabled,
                mode: preset.mode,
            })
            .collect::<Vec<_>>();
        pool_config.lru_enabled = scheduling_presets
            .iter()
            .any(|preset| preset.enabled && preset.preset.eq_ignore_ascii_case("lru"));
        pool_config.scheduling_presets = scheduling_presets;
    }

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
    if !local_candidate_failure_should_invalidate_affinity_for_provider(
        &context.plan.provider_api_format,
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
    if !local_candidate_failure_should_apply_key_effects(
        &context.plan.provider_api_format,
        effect.classification,
        effect.status_code,
    ) {
        return;
    }
    let Some(auth_config_fence) =
        capture_local_execution_auth_config_fence(state, context.plan).await
    else {
        return;
    };
    let effect_lock = PROVIDER_KEY_EFFECT_LOCKS.lock_for(&context.plan.key_id);
    let _effect_guard = effect_lock.lock().await;
    let observed_at_unix_secs = current_unix_secs();
    let current_rpm = state
        .read_recent_runtime_request_candidates(ADAPTIVE_RPM_RECENT_CANDIDATE_LIMIT)
        .await
        .ok()
        .map(|recent_candidates| {
            count_recent_rpm_requests_for_provider_key(
                &recent_candidates,
                &context.plan.key_id,
                observed_at_unix_secs,
            ) as u32
        });

    for _ in 0..PROVIDER_KEY_STATE_CAS_MAX_ATTEMPTS {
        let Some(current_key) = state
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&context.plan.key_id))
            .await
            .ok()
            .and_then(|mut keys| keys.drain(..).next())
        else {
            return;
        };
        if auth_config_fence
            .encrypted_auth_config()
            .is_some_and(|expected| current_key.encrypted_auth_config.as_deref() != Some(expected))
        {
            return;
        }
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
        let expected = ProviderCatalogKeyAdaptiveState::from(&current_key);
        let mut next = expected.clone();
        next.rpm_429_count = Some(projection.rpm_429_count);
        next.learned_rpm_limit = projection.learned_rpm_limit;
        next.last_429_at_unix_secs = Some(projection.last_429_at_unix_secs);
        next.last_429_type = Some(projection.last_429_type);
        next.adjustment_history = projection.adjustment_history;
        next.utilization_samples = projection.utilization_samples;
        next.last_probe_increase_at_unix_secs = projection.last_probe_increase_at_unix_secs;
        next.last_rpm_peak = projection.last_rpm_peak;
        let update = ProviderCatalogKeyAdaptiveStateUpdate {
            key_id: context.plan.key_id.clone(),
            expected_encrypted_auth_config: auth_config_fence
                .encrypted_auth_config()
                .map(ToOwned::to_owned),
            expected,
            next,
            status_snapshot_patch: adaptive_status_snapshot_patch(&projection.status_snapshot),
            updated_at_unix_secs: Some(observed_at_unix_secs),
        };
        provider_key_adaptive_success_persist_gate_reset(&context.plan.key_id);
        match state
            .compare_and_update_provider_catalog_key_adaptive_state(&update)
            .await
        {
            Ok(true) => return,
            Ok(false) => tokio::task::yield_now().await,
            Err(err) => {
                warn!(
                    "gateway orchestration effects: failed to persist adaptive rate-limit projection for provider {} endpoint {} key {}: {:?}",
                    context.plan.provider_id, context.plan.endpoint_id, context.plan.key_id, err
                );
                return;
            }
        }
    }
    warn!(
        "gateway orchestration effects: adaptive rate-limit CAS retries exhausted for provider {} endpoint {} key {}",
        context.plan.provider_id, context.plan.endpoint_id, context.plan.key_id
    );
}

fn adaptive_status_snapshot_patch(status_snapshot: &Value) -> Value {
    const OWNED_FIELDS: [&str; 6] = [
        "observation_count",
        "header_observation_count",
        "latest_upstream_limit",
        "learning_confidence",
        "enforcement_active",
        "known_boundary",
    ];
    let Some(snapshot) = status_snapshot.as_object() else {
        return serde_json::json!({});
    };
    Value::Object(
        OWNED_FIELDS
            .into_iter()
            .filter_map(|field| {
                snapshot
                    .get(field)
                    .cloned()
                    .map(|value| (field.to_string(), value))
            })
            .collect(),
    )
}

async fn record_adaptive_success_effect(
    state: &AppState,
    context: LocalExecutionEffectContext<'_>,
    _effect: LocalAdaptiveSuccessEffect,
) {
    let Some(auth_config_fence) =
        capture_local_execution_auth_config_fence(state, context.plan).await
    else {
        return;
    };
    let observed_at_unix_secs = current_unix_secs();
    let Some(current_key) = state
        .read_provider_catalog_keys_by_ids(std::slice::from_ref(&context.plan.key_id))
        .await
        .ok()
        .and_then(|mut keys| keys.drain(..).next())
    else {
        return;
    };
    if auth_config_fence
        .encrypted_auth_config()
        .is_some_and(|expected| current_key.encrypted_auth_config.as_deref() != Some(expected))
    {
        return;
    }
    if current_key.rpm_limit.is_some()
        || current_key
            .learned_rpm_limit
            .filter(|value| *value > 0)
            .is_none()
    {
        return;
    }
    let Some(gate_token) = provider_key_adaptive_success_persist_gate_admit(&context.plan.key_id)
    else {
        return;
    };

    let effect_lock = PROVIDER_KEY_EFFECT_LOCKS.lock_for(&context.plan.key_id);
    let _effect_guard = effect_lock.lock().await;
    if !provider_key_adaptive_success_persist_gate_admission_is_current(
        &context.plan.key_id,
        gate_token,
    ) {
        return;
    }
    let Some(recent_candidates) = state
        .read_recent_runtime_request_candidates(ADAPTIVE_RPM_RECENT_CANDIDATE_LIMIT)
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

    for _ in 0..PROVIDER_KEY_STATE_CAS_MAX_ATTEMPTS {
        let Some(current_key) = state
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&context.plan.key_id))
            .await
            .ok()
            .and_then(|mut keys| keys.drain(..).next())
        else {
            return;
        };
        if auth_config_fence
            .encrypted_auth_config()
            .is_some_and(|expected| current_key.encrypted_auth_config.as_deref() != Some(expected))
        {
            return;
        }
        if current_key.rpm_limit.is_some()
            || current_key
                .learned_rpm_limit
                .filter(|value| *value > 0)
                .is_none()
        {
            return;
        }
        let Some(projection) =
            project_local_adaptive_success(&current_key, current_rpm, observed_at_unix_secs)
        else {
            return;
        };
        let expected = ProviderCatalogKeyAdaptiveState::from(&current_key);
        let mut next = expected.clone();
        next.learned_rpm_limit = projection.learned_rpm_limit;
        next.adjustment_history = projection.adjustment_history;
        next.utilization_samples = projection.utilization_samples;
        next.last_probe_increase_at_unix_secs = projection.last_probe_increase_at_unix_secs;
        let update = ProviderCatalogKeyAdaptiveStateUpdate {
            key_id: context.plan.key_id.clone(),
            expected_encrypted_auth_config: auth_config_fence
                .encrypted_auth_config()
                .map(ToOwned::to_owned),
            expected,
            next,
            status_snapshot_patch: adaptive_status_snapshot_patch(&projection.status_snapshot),
            updated_at_unix_secs: Some(observed_at_unix_secs),
        };
        match state
            .compare_and_update_provider_catalog_key_adaptive_state(&update)
            .await
        {
            Ok(true) => return,
            Ok(false) => tokio::task::yield_now().await,
            Err(err) => {
                warn!(
                    "gateway orchestration effects: failed to persist adaptive success projection for provider {} endpoint {} key {}: {:?}",
                    context.plan.provider_id, context.plan.endpoint_id, context.plan.key_id, err
                );
                return;
            }
        }
    }
    warn!(
        "gateway orchestration effects: adaptive success CAS retries exhausted for provider {} endpoint {} key {}",
        context.plan.provider_id, context.plan.endpoint_id, context.plan.key_id
    );
}

fn provider_key_adaptive_success_persist_gate_admit(key_id: &str) -> Option<u64> {
    if cfg!(test) {
        return Some(0);
    }
    let interval = *ADAPTIVE_SUCCESS_PERSIST_MIN_INTERVAL;
    if interval.is_zero() {
        return Some(0);
    }
    let token = ADAPTIVE_SUCCESS_PERSIST_GATE_NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
    ADAPTIVE_SUCCESS_PERSIST_GATE
        .insert_if_absent_fresh(
            provider_key_adaptive_success_persist_gate_key(key_id),
            token,
            interval,
            ADAPTIVE_SUCCESS_PERSIST_GATE_MAX_ENTRIES,
        )
        .then_some(token)
}

fn provider_key_adaptive_success_persist_gate_admission_is_current(
    key_id: &str,
    token: u64,
) -> bool {
    if cfg!(test) || ADAPTIVE_SUCCESS_PERSIST_MIN_INTERVAL.is_zero() {
        return true;
    }
    ADAPTIVE_SUCCESS_PERSIST_GATE.get_fresh(
        &provider_key_adaptive_success_persist_gate_key(key_id),
        *ADAPTIVE_SUCCESS_PERSIST_MIN_INTERVAL,
    ) == Some(token)
}

fn provider_key_adaptive_success_persist_gate_reset(key_id: &str) {
    ADAPTIVE_SUCCESS_PERSIST_GATE.remove(&provider_key_adaptive_success_persist_gate_key(key_id));
}

fn provider_key_adaptive_success_persist_gate_key(key_id: &str) -> String {
    format!("adaptive-success:{key_id}")
}

async fn record_health_failure_effect(
    state: &AppState,
    context: LocalExecutionEffectContext<'_>,
    effect: LocalHealthFailureEffect,
) {
    if !local_candidate_failure_should_apply_key_effects(
        &context.plan.provider_api_format,
        effect.classification,
        effect.status_code,
    ) {
        return;
    }
    let api_format = context.plan.provider_api_format.trim();
    if api_format.is_empty() {
        return;
    }
    let Some(auth_config_fence) =
        capture_local_execution_auth_config_fence(state, context.plan).await
    else {
        return;
    };

    let effect_lock = PROVIDER_KEY_EFFECT_LOCKS.lock_for(&context.plan.key_id);
    let _effect_guard = effect_lock.lock().await;
    let is_pool_provider = local_execution_plan_uses_pool(state, context.plan).await;
    let observed_at_unix_secs = current_unix_secs();
    provider_key_health_success_persist_gate_reset(&context.plan.key_id, api_format);

    for _ in 0..PROVIDER_KEY_STATE_CAS_MAX_ATTEMPTS {
        let Some(current_key) = state
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&context.plan.key_id))
            .await
            .ok()
            .and_then(|mut keys| keys.drain(..).next())
        else {
            return;
        };
        if auth_config_fence
            .encrypted_auth_config()
            .is_some_and(|expected| current_key.encrypted_auth_config.as_deref() != Some(expected))
        {
            return;
        }
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
        let circuit_breaker_by_format = if is_pool_provider {
            None
        } else {
            project_local_key_circuit_failure(
                current_key.circuit_breaker_by_format.as_ref(),
                api_format,
                observed_at_unix_secs,
                consecutive_failures,
                current_key.max_probe_interval_minutes,
            )
            .or_else(|| current_key.circuit_breaker_by_format.clone())
        };
        let update = ProviderCatalogKeyHealthStateUpdate {
            key_id: context.plan.key_id.clone(),
            expected_encrypted_auth_config: auth_config_fence
                .encrypted_auth_config()
                .map(ToOwned::to_owned),
            expected_health_by_format: current_key.health_by_format,
            expected_circuit_breaker_by_format: current_key.circuit_breaker_by_format,
            health_by_format: Some(health_by_format),
            circuit_breaker_by_format,
        };
        match state
            .compare_and_update_provider_catalog_key_health_state(&update)
            .await
        {
            Ok(true) => return,
            Ok(false) => tokio::task::yield_now().await,
            Err(err) => {
                warn!(
                    "gateway orchestration effects: failed to persist health failure projection for provider {} endpoint {} key {}: {:?}",
                    context.plan.provider_id, context.plan.endpoint_id, context.plan.key_id, err
                );
                return;
            }
        }
    }
    warn!(
        "gateway orchestration effects: health failure CAS retries exhausted for provider {} endpoint {} key {}",
        context.plan.provider_id, context.plan.endpoint_id, context.plan.key_id
    );
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
    let Some(auth_config_fence) =
        capture_local_execution_auth_config_fence(state, context.plan).await
    else {
        return;
    };

    // Health updates replace both JSON snapshots in one write. Serialize the success
    // read/project/write with failure and circuit-clear effects for this provider key so a
    // stale success snapshot cannot overwrite a newer failure counter or open circuit.
    let effect_lock = PROVIDER_KEY_EFFECT_LOCKS.lock_for(&context.plan.key_id);
    let _effect_guard = effect_lock.lock().await;

    let is_pool_provider = local_execution_plan_uses_pool(state, context.plan).await;
    let mut persist_gate_checked = false;

    for _ in 0..PROVIDER_KEY_STATE_CAS_MAX_ATTEMPTS {
        let Some(current_key) = state
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&context.plan.key_id))
            .await
            .ok()
            .and_then(|mut keys| keys.drain(..).next())
        else {
            return;
        };
        if auth_config_fence
            .encrypted_auth_config()
            .is_some_and(|expected| current_key.encrypted_auth_config.as_deref() != Some(expected))
        {
            return;
        }
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
        if !persist_gate_checked {
            if !provider_key_health_success_persist_gate_allows(
                &context.plan.key_id,
                api_format,
                circuit_breaker_update_owned.is_some(),
            ) {
                return;
            }
            persist_gate_checked = true;
        }
        let circuit_breaker_by_format = if is_pool_provider {
            None
        } else {
            circuit_breaker_update_owned.or_else(|| current_key.circuit_breaker_by_format.clone())
        };
        let update = ProviderCatalogKeyHealthStateUpdate {
            key_id: context.plan.key_id.clone(),
            expected_encrypted_auth_config: auth_config_fence
                .encrypted_auth_config()
                .map(ToOwned::to_owned),
            expected_health_by_format: current_key.health_by_format,
            expected_circuit_breaker_by_format: current_key.circuit_breaker_by_format,
            health_by_format: Some(health_by_format),
            circuit_breaker_by_format,
        };
        match state
            .compare_and_update_provider_catalog_key_health_state(&update)
            .await
        {
            Ok(true) => return,
            Ok(false) => tokio::task::yield_now().await,
            Err(err) => {
                warn!(
                    "gateway orchestration effects: failed to persist health success projection for provider {} endpoint {} key {}: {:?}",
                    context.plan.provider_id, context.plan.endpoint_id, context.plan.key_id, err
                );
                return;
            }
        }
    }
    warn!(
        "gateway orchestration effects: health success CAS retries exhausted for provider {} endpoint {} key {}",
        context.plan.provider_id, context.plan.endpoint_id, context.plan.key_id
    );
}

fn provider_key_health_success_persist_gate_allows(
    key_id: &str,
    api_format: &str,
    closes_circuit: bool,
) -> bool {
    if closes_circuit {
        return true;
    }
    let min_interval = *HEALTH_SUCCESS_PERSIST_MIN_INTERVAL;
    if min_interval.is_zero() {
        return true;
    }
    let key = provider_key_health_success_persist_gate_key(key_id, api_format);
    HEALTH_SUCCESS_PERSIST_GATE.insert_if_absent_fresh(
        key,
        (),
        min_interval,
        HEALTH_SUCCESS_PERSIST_GATE_MAX_ENTRIES,
    )
}

fn provider_key_health_success_persist_gate_reset(key_id: &str, api_format: &str) {
    let key = provider_key_health_success_persist_gate_key(key_id, api_format);
    HEALTH_SUCCESS_PERSIST_GATE.remove(&key);
}

fn provider_key_health_success_persist_gate_key(key_id: &str, api_format: &str) -> String {
    format!("success:{key_id}:{api_format}")
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
    if !local_candidate_failure_should_apply_key_effects(
        &context.plan.provider_api_format,
        effect.classification,
        effect.status_code,
    ) {
        return;
    }
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
    if capture_local_execution_auth_config_fence(state, context.plan)
        .await
        .is_none()
    {
        return;
    }
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
    let Some(auth_config_fence) =
        capture_local_execution_auth_config_fence(state, context.plan).await
    else {
        return;
    };
    let effect_lock = PROVIDER_KEY_EFFECT_LOCKS.lock_for(&context.plan.key_id);
    let _effect_guard = effect_lock.lock().await;

    for _ in 0..PROVIDER_KEY_STATE_CAS_MAX_ATTEMPTS {
        let Some(current_key) = state
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&context.plan.key_id))
            .await
            .ok()
            .and_then(|mut keys| keys.drain(..).next())
        else {
            return;
        };
        if auth_config_fence
            .encrypted_auth_config()
            .is_some_and(|expected| current_key.encrypted_auth_config.as_deref() != Some(expected))
        {
            return;
        }
        if current_key.circuit_breaker_by_format.is_none() {
            return;
        }
        let update = ProviderCatalogKeyHealthStateUpdate {
            key_id: context.plan.key_id.clone(),
            expected_encrypted_auth_config: auth_config_fence
                .encrypted_auth_config()
                .map(ToOwned::to_owned),
            expected_health_by_format: current_key.health_by_format.clone(),
            expected_circuit_breaker_by_format: current_key.circuit_breaker_by_format,
            health_by_format: current_key.health_by_format,
            circuit_breaker_by_format: None,
        };
        match state
            .compare_and_update_provider_catalog_key_health_state(&update)
            .await
        {
            Ok(true) => return,
            Ok(false) => tokio::task::yield_now().await,
            Err(err) => {
                warn!(
                    "gateway orchestration effects: failed to clear pool key circuit for provider {} endpoint {} key {}: {:?}",
                    context.plan.provider_id, context.plan.endpoint_id, context.plan.key_id, err
                );
                return;
            }
        }
    }
    warn!(
        "gateway orchestration effects: clear pool key circuit CAS retries exhausted for provider {} endpoint {} key {}",
        context.plan.provider_id, context.plan.endpoint_id, context.plan.key_id
    );
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
    // Agent assertions are long-lived credential requests whose task can rotate
    // while the response is in flight. Runtime 401/403 handling must not project
    // that response onto whichever credential generation is stored later.
    if execution_plan_uses_codex_agent_identity(plan) {
        return;
    }
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
    // The inverse replacement is equally unsafe: a response sent with an old
    // bearer token must not invalidate a newly installed Agent Identity.
    if crate::provider_transport::is_codex_agent_identity_transport(&transport) {
        return;
    }
    if !transport.key.auth_type.trim().eq_ignore_ascii_case("oauth") {
        return;
    }
    if !execution_plan_bearer_matches_transport(plan, &transport) {
        return;
    }

    let Some(invalid_reason) = resolve_local_oauth_invalid_reason(
        transport.provider.provider_type.as_str(),
        effect.status_code,
        effect.response_text,
    ) else {
        return;
    };
    let request_started_at_unix_ms = report_context_u64_field(
        context.report_context,
        "provider_request_started_at_unix_ms",
    );
    let request_order_id =
        report_context_string_field(context.report_context, "provider_request_order_id");
    let observed_credential_generation =
        report_context_string_field(context.report_context, "codex_credential_generation");

    if let Err(err) = state
        .mark_provider_transport_oauth_invalid_fenced(
            &transport,
            invalid_reason.as_str(),
            request_started_at_unix_ms,
            request_order_id,
            observed_credential_generation,
        )
        .await
    {
        warn!(
            "gateway orchestration effects: failed to persist oauth invalidation for provider {} endpoint {} key {}: {:?}",
            plan.provider_id, plan.endpoint_id, plan.key_id, err
        );
    }
}

async fn record_oauth_success_effect(
    state: &AppState,
    context: LocalExecutionEffectContext<'_>,
    effect: LocalOAuthSuccessEffect<'_>,
) {
    let Some(effect) =
        owned_local_oauth_success_effect(context.plan, context.report_context, effect)
    else {
        return;
    };
    record_oauth_success_effect_owned(state, effect).await;
}

async fn record_oauth_success_effect_owned(state: &AppState, effect: OwnedLocalOAuthSuccessEffect) {
    if !(200..300).contains(&effect.status_code) {
        return;
    }
    let transport = match state
        .read_provider_transport_snapshot(&effect.provider_id, &effect.endpoint_id, &effect.key_id)
        .await
    {
        Ok(Some(transport)) => transport,
        Ok(None) => return,
        Err(err) => {
            warn!(
                "gateway orchestration effects: failed to read transport snapshot for oauth success provider {} endpoint {} key {}: {:?}",
                effect.provider_id, effect.endpoint_id, effect.key_id, err
            );
            return;
        }
    };
    if !transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("codex")
        || crate::provider_transport::is_codex_agent_identity_transport(&transport)
        || !transport.key.auth_type.trim().eq_ignore_ascii_case("oauth")
        || crate::provider_transport::resolve_local_generic_oauth_transport_authorization(
            &transport,
        )
        .as_deref()
        .and_then(bearer_access_token)
            != bearer_access_token(effect.authorization.as_str())
    {
        return;
    }

    if let Err(err) = state
        .mark_provider_transport_oauth_success_fenced(
            &transport,
            Some(effect.request_started_at_unix_ms),
            Some(effect.request_order_id.as_str()),
            effect.observed_credential_generation.as_deref(),
        )
        .await
    {
        warn!(
            "gateway orchestration effects: failed to persist oauth success for provider {} endpoint {} key {}: {:?}",
            effect.provider_id, effect.endpoint_id, effect.key_id, err
        );
    }
}

fn execution_plan_uses_codex_agent_identity(plan: &ExecutionPlan) -> bool {
    execution_plan_authorization(plan)
        .is_some_and(crate::provider_transport::is_codex_agent_identity_authorization)
}

fn execution_plan_authorization(plan: &ExecutionPlan) -> Option<&str> {
    plan.headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
        .map(|(_, value)| value.as_str())
}

fn execution_plan_bearer_matches_transport(
    plan: &ExecutionPlan,
    transport: &crate::provider_transport::GatewayProviderTransportSnapshot,
) -> bool {
    let Some(plan_token) = execution_plan_authorization(plan).and_then(bearer_access_token) else {
        return false;
    };
    crate::provider_transport::resolve_local_generic_oauth_transport_authorization(transport)
        .as_deref()
        .and_then(bearer_access_token)
        .is_some_and(|current_token| current_token == plan_token)
}

fn bearer_access_token(authorization: &str) -> Option<&str> {
    let mut parts = authorization.split_ascii_whitespace();
    let scheme = parts.next()?;
    let token = parts.next()?;
    (scheme.eq_ignore_ascii_case("bearer") && parts.next().is_none()).then_some(token)
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
        _ if super::oauth_status_may_be_invalid(status_code, response_text) => Some(format!(
            "[OAUTH_EXPIRED] {}",
            upstream_message
                .as_deref()
                .unwrap_or("OAuth access token was rejected")
        )),
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
        | LocalFailoverClassification::StopExecutionError
        | LocalFailoverClassification::StopCyberPolicy => false,
    }
}

fn local_candidate_failure_should_invalidate_affinity_for_provider(
    provider_api_format: &str,
    classification: LocalFailoverClassification,
    status_code: u16,
) -> bool {
    if !local_candidate_failure_should_invalidate_affinity(classification, status_code) {
        return false;
    }
    if !provider_api_format
        .trim()
        .eq_ignore_ascii_case("claude:messages")
    {
        return true;
    }

    let disposition =
        classify_failure_disposition(provider_api_format, classification, status_code);
    !(disposition.retry_action == crate::orchestration::FailureRetryAction::Stop
        && disposition.failure_scope == FailureScope::None)
}

fn local_candidate_failure_should_apply_key_effects(
    provider_api_format: &str,
    classification: LocalFailoverClassification,
    status_code: u16,
) -> bool {
    if !provider_api_format
        .trim()
        .eq_ignore_ascii_case("claude:messages")
    {
        return true;
    }

    matches!(
        classify_failure_disposition(provider_api_format, classification, status_code)
            .failure_scope,
        FailureScope::Credential
    )
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
    if capture_local_execution_auth_config_fence(state, context.plan)
        .await
        .is_none()
    {
        return;
    }
    if !pool_score_feedback_gate_allows(context.plan, succeeded, hard_state, score_delta) {
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

fn pool_score_feedback_interval_from_env(key: &str, default_secs: u64) -> Duration {
    let secs = std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(default_secs)
        .min(MAX_POOL_SCORE_FEEDBACK_MIN_INTERVAL_SECS);
    Duration::from_secs(secs)
}

fn pool_score_feedback_min_interval(succeeded: Option<bool>) -> Duration {
    match succeeded {
        Some(false) => *POOL_SCORE_FAILURE_FEEDBACK_MIN_INTERVAL,
        _ => *POOL_SCORE_SUCCESS_FEEDBACK_MIN_INTERVAL,
    }
}

fn pool_score_feedback_gate_key(
    plan: &ExecutionPlan,
    succeeded: Option<bool>,
    hard_state: Option<PoolMemberHardState>,
    score_delta: Option<i32>,
) -> String {
    let succeeded = match succeeded {
        Some(true) => "success",
        Some(false) => "failure",
        None => "neutral",
    };
    let hard_state = hard_state
        .map(PoolMemberHardState::as_database)
        .unwrap_or("none");
    format!(
        "provider:{}:key:{}:result:{}:state:{}:delta:{}",
        plan.provider_id,
        plan.key_id,
        succeeded,
        hard_state,
        score_delta.unwrap_or_default()
    )
}

fn pool_score_feedback_gate_allows(
    plan: &ExecutionPlan,
    succeeded: Option<bool>,
    hard_state: Option<PoolMemberHardState>,
    score_delta: Option<i32>,
) -> bool {
    let min_interval = pool_score_feedback_min_interval(succeeded);
    if min_interval.is_zero() {
        return true;
    }
    let key = pool_score_feedback_gate_key(plan, succeeded, hard_state, score_delta);
    if POOL_SCORE_FEEDBACK_GATE.contains_fresh(&key, min_interval) {
        return false;
    }
    POOL_SCORE_FEEDBACK_GATE.insert(key, (), min_interval, POOL_SCORE_FEEDBACK_GATE_MAX_ENTRIES);
    true
}

fn pool_score_hard_state_for_status(
    status_code: u16,
    error_body: Option<&str>,
) -> Option<PoolMemberHardState> {
    if let Some(reason) = admin_provider_pool_key_terminal_error_reason(status_code, error_body) {
        return Some(pool_score_hard_state_for_terminal_error_reason(&reason));
    }

    // A number of providers report account quota exhaustion as HTTP 429 rather
    // than 402. Keep those members out of the score-based pool fallback until
    // the provider's quota probe observes a reset; treating every 429 as a
    // generic cooldown otherwise lets the member re-enter as soon as the short
    // transient cooldown expires.
    if status_code == 429 && error_body_indicates_quota_exhaustion(error_body) {
        return Some(PoolMemberHardState::QuotaExhausted);
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

fn error_body_indicates_quota_exhaustion(error_body: Option<&str>) -> bool {
    let body = error_body.unwrap_or_default().to_ascii_lowercase();
    [
        "quota exhausted",
        "quota_exhausted",
        "quota exceeded",
        "quota_exceeded",
        "insufficient_quota",
        "resource exhausted",
        "resource has been exhausted",
        "resource_exhausted",
        "usage_limit_reached",
        "limit_reached",
        "quota limit reached",
        "credits exhausted",
        "insufficient credits",
    ]
    .iter()
    .any(|marker| body.contains(marker))
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
        ProviderCatalogKeyAdaptiveState, StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
        StoredProviderCatalogProvider,
    };
    use aether_test_support::ManagedRedisServer;
    use aether_usage_runtime::GatewaySyncReportRequest;
    use serde_json::{json, Value};

    use super::{
        apply_local_execution_effect, apply_local_stream_failure_effects,
        apply_local_stream_success_effects, execution_plan_bearer_matches_transport,
        local_candidate_failure_should_apply_key_effects,
        local_candidate_failure_should_record_pool_error, pool_score_feedback_gate_allows,
        pool_score_hard_state_for_status, resolve_pool_feedback_context,
        LocalAdaptiveRateLimitEffect, LocalAdaptiveSuccessEffect, LocalAttemptFailureEffect,
        LocalExecutionEffect, LocalExecutionEffectContext, LocalHealthFailureEffect,
        LocalHealthSuccessEffect, LocalOAuthInvalidationEffect, LocalOAuthSuccessEffect,
        LocalPoolErrorEffect, LocalStreamFailureEffect, ProviderKeyEffectLockPool,
    };
    use crate::data::{GatewayDataConfig, GatewayDataState};
    use crate::orchestration::{
        apply_local_report_effect, LocalFailoverClassification, LocalReportEffect,
    };
    use crate::scheduler::affinity::SCHEDULER_AFFINITY_TTL;
    use crate::usage::GatewayStreamReportRequest;
    use crate::AppState;
    use aether_scheduler_core::{
        build_scheduler_affinity_cache_key_for_api_key_id,
        build_scheduler_affinity_cache_key_for_api_key_id_with_client_session,
        build_scheduler_affinity_cache_key_for_api_key_id_with_client_session_and_scope,
        ClientSessionAffinity, SchedulerAffinityScope, SchedulerAffinityTarget,
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

    fn sample_claude_plan() -> ExecutionPlan {
        let mut plan = sample_plan();
        plan.provider_name = Some("anthropic".to_string());
        plan.provider_api_format = "claude:messages".to_string();
        plan
    }

    fn sample_stream_report() -> GatewayStreamReportRequest {
        GatewayStreamReportRequest {
            trace_id: "trace-stream-effects".to_string(),
            report_kind: "openai_chat_stream_success".to_string(),
            report_context: None,
            status_code: 200,
            headers: BTreeMap::new(),
            provider_body_base64: None,
            provider_body_state: None,
            client_body_base64: None,
            client_body_state: None,
            terminal_summary: None,
            telemetry: None,
        }
    }

    #[test]
    fn pool_score_feedback_gate_suppresses_repeated_success_writes() {
        super::POOL_SCORE_FEEDBACK_GATE.clear();
        let plan = sample_plan();

        assert!(pool_score_feedback_gate_allows(
            &plan,
            Some(true),
            Some(PoolMemberHardState::Available),
            Some(50),
        ));
        assert!(!pool_score_feedback_gate_allows(
            &plan,
            Some(true),
            Some(PoolMemberHardState::Available),
            Some(50),
        ));
        assert!(pool_score_feedback_gate_allows(
            &plan,
            Some(false),
            Some(PoolMemberHardState::Cooldown),
            Some(-500),
        ));
    }

    #[test]
    fn provider_key_effect_lock_pool_prunes_geometrically_and_keeps_active_locks() {
        let pool = ProviderKeyEffectLockPool::new(4);
        let locks = (0..10)
            .map(|index| pool.lock_for(&format!("key-{index}")))
            .collect::<Vec<_>>();

        {
            let state = pool.state.lock().expect("effect lock pool should lock");
            assert_eq!(state.entries.len(), 10);
            assert_eq!(state.prune_count, 2);
            assert_eq!(state.next_growth_prune_at, 16);
        }

        for (index, expected) in locks.iter().enumerate() {
            let existing = pool.lock_for(&format!("key-{index}"));
            assert!(Arc::ptr_eq(expected, &existing));
        }

        let hot_lock = Arc::clone(&locks[0]);
        drop(locks);
        for _ in 0..32 {
            let existing = pool.lock_for("key-0");
            assert!(Arc::ptr_eq(&hot_lock, &existing));
        }

        let state = pool.state.lock().expect("effect lock pool should lock");
        assert_eq!(state.entries.len(), 1);
        assert!(state.prune_count >= 3);
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
            "scheduler_affinity_policy": {
                "scheduling_mode": "cache_affinity"
            },
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

    fn cache_affinity_report_context() -> Value {
        json!({
            "api_key_id": "api-key-1",
            "client_api_format": "openai:chat",
            "model": "gpt-5",
            "scheduler_affinity_policy": {
                "scheduling_mode": "cache_affinity"
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
            headers: BTreeMap::from([(
                "authorization".to_string(),
                "Bearer codex-access-token".to_string(),
            )]),
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

    fn sample_codex_agent_identity_plan() -> ExecutionPlan {
        let mut plan = sample_codex_plan();
        plan.headers.insert(
            "Authorization".to_string(),
            "AgentAssertion in-flight-assertion".to_string(),
        );
        plan
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

    fn sample_codex_provider_with_auto_remove() -> StoredProviderCatalogProvider {
        let mut provider = sample_codex_provider();
        provider.config = Some(json!({
            "pool_advanced": {
                "auto_remove_banned_keys": true
            }
        }));
        provider
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
        let provider_id = "provider-codex-cli-local-1";
        let key_id = "key-codex-cli-local-1";
        let credential_state = AppState::new()
            .expect("credential state should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled()
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            );
        let encrypted_api_key = credential_state
            .seal_provider_catalog_key_api_key(provider_id, key_id, "codex-access-token")
            .expect("access token should encrypt");
        let encrypted_auth_config = credential_state
            .seal_provider_catalog_key_auth_config(
                provider_id,
                key_id,
                r#"{"provider_type":"codex","refresh_token":"rt-codex-local-123"}"#,
            )
            .expect("auth config should encrypt");
        StoredProviderCatalogKey::new(
            key_id.to_string(),
            provider_id.to_string(),
            "oauth".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(serde_json::json!(["openai:responses"])),
            encrypted_api_key,
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

    fn sample_codex_agent_identity_key() -> StoredProviderCatalogKey {
        let mut key = sample_codex_key();
        key.name = "Agent Identity".to_string();
        key.encrypted_api_key = Some(
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "__placeholder__")
                .expect("placeholder api key should encrypt"),
        );
        key.encrypted_auth_config = Some(
            encrypt_python_fernet_plaintext(
                DEVELOPMENT_ENCRYPTION_KEY,
                r#"{"provider_type":"codex","auth_mode":"agentIdentity","agent_runtime_id":"runtime-current","agent_private_key":"MC4CAQAwBQYDK2VwBCIEIAcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcH","task_id":"task-current"}"#,
            )
            .expect("Agent Identity auth config should encrypt"),
        );
        key
    }

    fn codex_state() -> AppState {
        codex_state_with_provider(sample_codex_provider())
    }

    fn codex_state_with_auto_remove() -> AppState {
        codex_state_with_provider(sample_codex_provider_with_auto_remove())
    }

    fn codex_state_with_provider(provider: StoredProviderCatalogProvider) -> AppState {
        codex_state_with_provider_and_key(provider, sample_codex_key())
    }

    fn codex_state_with_provider_and_key(
        provider: StoredProviderCatalogProvider,
        key: StoredProviderCatalogKey,
    ) -> AppState {
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![provider],
            vec![sample_codex_endpoint()],
            vec![key],
        ));
        AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(repository)
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
    }

    fn claude_code_oauth_state() -> AppState {
        let mut provider = sample_codex_provider();
        provider.name = "claude_code".to_string();
        provider.provider_type = "claude_code".to_string();
        let mut endpoint = sample_codex_endpoint();
        endpoint.api_format = "claude:messages".to_string();
        endpoint.api_family = Some("claude".to_string());
        endpoint.base_url = "https://api.anthropic.com".to_string();
        let mut key = sample_codex_key();
        key.api_formats = Some(json!(["claude:messages"]));
        key.encrypted_auth_config = Some(
            encrypt_python_fernet_plaintext(
                DEVELOPMENT_ENCRYPTION_KEY,
                r#"{"provider_type":"claude_code","refresh_token":"rt-claude-local-123"}"#,
            )
            .expect("Claude Code auth config should encrypt"),
        );
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![provider],
            vec![endpoint],
            vec![key],
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

    #[tokio::test]
    async fn pool_feedback_uses_routing_profile_scheduling_override() {
        let mut provider = sample_pool_health_provider();
        provider.config = Some(json!({
            "pool_advanced": {
                "scheduling_presets": [{
                    "preset": "lru",
                    "enabled": true
                }]
            }
        }));
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![provider],
            vec![sample_health_endpoint()],
            vec![sample_health_key()],
        ));
        let state = AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(repository)
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            );
        let plan = sample_plan();
        let report_context = json!({
            "routing_pool_policy_override": {
                "scheduling_presets": [{
                    "preset": "cache_affinity",
                    "enabled": true
                }]
            }
        });

        let feedback = resolve_pool_feedback_context(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: Some(&report_context),
            },
        )
        .await
        .expect("pool feedback context should resolve");

        assert!(
            crate::handlers::shared::provider_pool::admin_provider_pool_cache_affinity_enabled(
                &feedback.pool_config
            )
        );
        assert!(!feedback.pool_config.lru_enabled);
        assert_eq!(feedback.pool_config.scheduling_presets.len(), 1);
        assert_eq!(
            feedback.pool_config.scheduling_presets[0].preset,
            "cache_affinity"
        );
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
    async fn stream_success_effect_helper_projects_health_and_scheduler_affinity() {
        let state = AppState::new().expect("gateway state should build");
        let plan = sample_plan();
        let report_context = cache_affinity_report_context();
        let cache_key =
            build_scheduler_affinity_cache_key_for_api_key_id("api-key-1", "openai:chat", "gpt-5")
                .expect("scheduler affinity cache key should build");
        let payload = sample_stream_report();

        apply_local_stream_success_effects(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: Some(&report_context),
            },
            &payload,
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
    async fn stream_failure_effect_helper_returns_analysis_and_projects_health() {
        let state = health_state();
        let plan = sample_plan();
        let headers = BTreeMap::new();
        let analysis = apply_local_stream_failure_effects(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: None,
            },
            LocalStreamFailureEffect::new(503, &headers, Some("upstream unavailable"))
                .with_stream_timeout(),
        )
        .await;

        assert_eq!(
            analysis.classification,
            LocalFailoverClassification::UseDefault
        );
        assert_eq!(analysis.decision.as_str(), "use_default");
        let stored_key = state
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&plan.key_id))
            .await
            .expect("provider catalog keys should load")
            .into_iter()
            .next()
            .expect("stored key should exist");
        assert_eq!(
            stored_key
                .health_by_format
                .as_ref()
                .and_then(|value| value.get("openai:chat"))
                .and_then(|value| value.get("consecutive_failures"))
                .and_then(Value::as_u64),
            Some(1)
        );
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
        let report_context = cache_affinity_report_context();
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
    async fn routing_profile_cache_affinity_overrides_legacy_fixed_mode_on_success() {
        let state = AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled().with_system_config_values_for_tests(vec![(
                    "scheduling_mode".to_string(),
                    json!("fixed_order"),
                )]),
            );
        let plan = sample_plan();
        let affinity = session_affinity();
        let scope = SchedulerAffinityScope::new("routing-group-1", Some(7));
        let report_context = json!({
            "api_key_id": "api-key-1",
            "client_api_format": "openai:chat",
            "model": "gpt-5",
            "client_session_affinity": {
                "client_family": "generic",
                "session_key": "session=session-1;agent=coder"
            },
            "scheduler_affinity_policy": {
                "scheduling_mode": "cache_affinity",
                "scope": {
                    "routing_group_id": "routing-group-1",
                    "routing_group_version": 7
                }
            }
        });
        let scoped_cache_key =
            build_scheduler_affinity_cache_key_for_api_key_id_with_client_session_and_scope(
                "api-key-1",
                "openai:chat",
                "gpt-5",
                Some(&affinity),
                Some(&scope),
            )
            .expect("scoped scheduler affinity cache key should build");

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
            state.read_scheduler_affinity_target(scoped_cache_key.as_str(), SCHEDULER_AFFINITY_TTL),
            Some(SchedulerAffinityTarget {
                provider_id: "prov-1".to_string(),
                endpoint_id: "ep-1".to_string(),
                key_id: "key-1".to_string(),
            })
        );
        assert!(state
            .read_scheduler_affinity_target(
                session_scheduler_affinity_cache_key().as_str(),
                SCHEDULER_AFFINITY_TTL
            )
            .is_none());
    }

    #[tokio::test]
    async fn routing_profile_fixed_mode_overrides_legacy_cache_affinity_on_success() {
        let state = AppState::new().expect("gateway state should build");
        let plan = sample_plan();
        let report_context = json!({
            "api_key_id": "api-key-1",
            "client_api_format": "openai:chat",
            "model": "gpt-5",
            "scheduler_affinity_policy": {
                "scheduling_mode": "fixed_order",
                "scope": {
                    "routing_group_id": "routing-group-1",
                    "routing_group_version": 7
                }
            }
        });
        let scope = SchedulerAffinityScope::new("routing-group-1", Some(7));
        let scoped_cache_key =
            build_scheduler_affinity_cache_key_for_api_key_id_with_client_session_and_scope(
                "api-key-1",
                "openai:chat",
                "gpt-5",
                None,
                Some(&scope),
            )
            .expect("scoped scheduler affinity cache key should build");

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
            .read_scheduler_affinity_target(scoped_cache_key.as_str(), SCHEDULER_AFFINITY_TTL)
            .is_none());
    }

    #[tokio::test]
    async fn malformed_routing_affinity_context_does_not_fall_back_to_legacy_mode() {
        let state = AppState::new().expect("gateway state should build");
        let plan = sample_plan();
        let report_context = json!({
            "api_key_id": "api-key-1",
            "client_api_format": "openai:chat",
            "model": "gpt-5",
            "scheduler_affinity_policy": {
                "scheduling_mode": "unknown"
            }
        });
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

        assert!(state
            .read_scheduler_affinity_target(legacy_cache_key.as_str(), SCHEDULER_AFFINITY_TTL)
            .is_none());
    }

    #[tokio::test]
    async fn health_success_keeps_scheduler_affinity_after_health_state_update() {
        let state = health_state();
        let plan = sample_plan();
        let report_context = cache_affinity_report_context();
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
        let state = AppState::new().expect("gateway state should build");
        let plan = sample_plan();
        let report_context = json!({
            "api_key_id": "api-key-1",
            "client_api_format": "openai:chat",
            "model": "gpt-5",
            "scheduler_affinity_policy": {
                "scheduling_mode": "load_balance"
            }
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
        let report_context = cache_affinity_report_context();
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
    fn anthropic_non_credential_failures_do_not_apply_key_wide_effects() {
        assert!(!local_candidate_failure_should_apply_key_effects(
            "claude:messages",
            LocalFailoverClassification::RetryUpstreamFailure,
            529,
        ));
        assert!(!local_candidate_failure_should_apply_key_effects(
            "claude:messages",
            LocalFailoverClassification::RetryUpstreamFailure,
            429,
        ));
        assert!(!local_candidate_failure_should_apply_key_effects(
            "claude:messages",
            LocalFailoverClassification::RetryUpstreamFailure,
            503,
        ));
        assert!(local_candidate_failure_should_apply_key_effects(
            "claude:messages",
            LocalFailoverClassification::RetryUpstreamFailure,
            401,
        ));
        assert!(local_candidate_failure_should_apply_key_effects(
            "claude:messages",
            LocalFailoverClassification::RetryUpstreamFailure,
            403,
        ));
        assert!(!local_candidate_failure_should_apply_key_effects(
            "claude:messages",
            LocalFailoverClassification::RetryUpstreamFailure,
            400,
        ));
        assert!(local_candidate_failure_should_apply_key_effects(
            "openai:chat",
            LocalFailoverClassification::RetryUpstreamFailure,
            529,
        ));
        assert!(local_candidate_failure_should_apply_key_effects(
            "openai:chat",
            LocalFailoverClassification::RetryUpstreamFailure,
            429,
        ));
        assert!(local_candidate_failure_should_apply_key_effects(
            "openai:chat",
            LocalFailoverClassification::RetryUpstreamFailure,
            503,
        ));
    }

    #[tokio::test]
    async fn anthropic_non_credential_failures_preserve_key_wide_state() {
        for status_code in [400, 429, 503, 529] {
            let mut key = sample_adaptive_key();
            let circuit = json!({
                "openai:chat": {
                    "open": true,
                    "reason": "existing-state"
                }
            });
            key.circuit_breaker_by_format = Some(circuit.clone());
            let expected_adaptive_state = ProviderCatalogKeyAdaptiveState::from(&key);
            let expected_health = key.health_by_format.clone();
            let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
                vec![sample_pool_health_provider()],
                vec![sample_health_endpoint()],
                vec![key],
            ));
            let state = AppState::new()
                .expect("gateway state should build")
                .with_data_state_for_tests(
                    GatewayDataState::with_provider_catalog_repository_for_tests(repository)
                        .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
                );
            let plan = sample_claude_plan();
            let report_context = json!({
                "api_key_id": "api-key-1",
                "client_api_format": "claude:messages",
                "model": "claude-sonnet-4-5",
            });
            let cache_key = build_scheduler_affinity_cache_key_for_api_key_id(
                "api-key-1",
                "claude:messages",
                "claude-sonnet-4-5",
            )
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
            let headers = BTreeMap::from([("Retry-After".to_string(), "120".to_string())]);
            let context = LocalExecutionEffectContext {
                plan: &plan,
                report_context: Some(&report_context),
            };
            let classification = LocalFailoverClassification::RetryUpstreamFailure;

            apply_local_execution_effect(
                &state,
                context,
                LocalExecutionEffect::AttemptFailure(LocalAttemptFailureEffect {
                    status_code,
                    classification,
                }),
            )
            .await;
            apply_local_execution_effect(
                &state,
                context,
                LocalExecutionEffect::AdaptiveRateLimit(LocalAdaptiveRateLimitEffect {
                    status_code,
                    classification,
                    headers: Some(&headers),
                }),
            )
            .await;
            apply_local_execution_effect(
                &state,
                context,
                LocalExecutionEffect::HealthFailure(LocalHealthFailureEffect {
                    status_code,
                    classification,
                }),
            )
            .await;
            apply_local_execution_effect(
                &state,
                context,
                LocalExecutionEffect::PoolError(LocalPoolErrorEffect {
                    status_code,
                    classification,
                    headers: &headers,
                    error_body: Some(r#"{"error":{"message":"temporarily unavailable"}}"#),
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
                ProviderCatalogKeyAdaptiveState::from(&stored_key),
                expected_adaptive_state,
                "Anthropic status {status_code} must not update key-wide adaptive state"
            );
            assert_eq!(
                stored_key.health_by_format, expected_health,
                "Anthropic status {status_code} must not update key-wide health"
            );
            assert_eq!(
                stored_key.circuit_breaker_by_format,
                Some(circuit),
                "Anthropic status {status_code} must not clear pool key state"
            );
            let expected_affinity = (status_code == 400).then_some(target);
            assert_eq!(
                state.read_scheduler_affinity_target(&cache_key, SCHEDULER_AFFINITY_TTL),
                expected_affinity,
                "Anthropic status {status_code} must invalidate only retryable target affinity"
            );
        }
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
        assert_eq!(
            pool_score_hard_state_for_status(
                429,
                Some(r#"{"error":{"status":"RESOURCE_EXHAUSTED","message":"quota exhausted"}}"#),
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
    async fn oauth_bearer_generation_match_supports_generic_auth_config_token() {
        let state = codex_state();
        let mut transport = state
            .read_provider_transport_snapshot(
                "provider-codex-cli-local-1",
                "endpoint-codex-cli-local-1",
                "key-codex-cli-local-1",
            )
            .await
            .expect("transport should load")
            .expect("transport should exist");
        transport.provider.provider_type = "claude_code".to_string();
        transport.key.decrypted_api_key = "__placeholder__".to_string();
        transport.key.decrypted_auth_config =
            Some(json!({"accessToken": "current-access-token"}).to_string());
        let mut plan = sample_codex_plan();
        plan.headers.insert(
            "authorization".to_string(),
            "Bearer current-access-token".to_string(),
        );
        assert!(execution_plan_bearer_matches_transport(&plan, &transport));

        plan.headers.insert(
            "authorization".to_string(),
            "Bearer stale-access-token".to_string(),
        );
        assert!(!execution_plan_bearer_matches_transport(&plan, &transport));

        transport.key.decrypted_api_key = "replacement-access-token".to_string();
        plan.headers.insert(
            "authorization".to_string(),
            "Bearer current-access-token".to_string(),
        );
        assert!(!execution_plan_bearer_matches_transport(&plan, &transport));
        plan.headers.insert(
            "authorization".to_string(),
            "Bearer replacement-access-token".to_string(),
        );
        assert!(execution_plan_bearer_matches_transport(&plan, &transport));

        transport.key.decrypted_auth_config = Some(
            json!({
                "accessToken": "current-access-token",
                "request": {
                    "extraHeaders": {
                        "Authorization": "Bearer nested-override-token"
                    }
                }
            })
            .to_string(),
        );
        plan.headers.insert(
            "authorization".to_string(),
            "Bearer replacement-access-token".to_string(),
        );
        assert!(!execution_plan_bearer_matches_transport(&plan, &transport));
        plan.headers.insert(
            "authorization".to_string(),
            "Bearer nested-override-token".to_string(),
        );
        assert!(execution_plan_bearer_matches_transport(&plan, &transport));
    }

    #[tokio::test]
    async fn oauth_invalidation_marks_claude_code_authentication_failures_only() {
        let state = claude_code_oauth_state();
        let mut plan = sample_codex_plan();
        plan.provider_name = Some("claude_code".to_string());
        plan.provider_api_format = "claude:messages".to_string();

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: None,
            },
            LocalExecutionEffect::OauthInvalidation(LocalOAuthInvalidationEffect {
                status_code: 403,
                response_text: Some(
                    r#"{"type":"error","error":{"type":"permission_error","message":"insufficient scope"}}"#,
                ),
            }),
        )
        .await;
        let unmarked = state
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&plan.key_id))
            .await
            .expect("provider catalog keys should load")
            .into_iter()
            .next()
            .expect("stored key should exist");
        assert!(unmarked.oauth_invalid_at_unix_secs.is_none());

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: None,
            },
            LocalExecutionEffect::OauthInvalidation(LocalOAuthInvalidationEffect {
                status_code: 403,
                response_text: Some(
                    r#"{"type":"error","error":{"type":"authentication_error","message":"invalid access token"}}"#,
                ),
            }),
        )
        .await;
        let marked = state
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&plan.key_id))
            .await
            .expect("provider catalog keys should load")
            .into_iter()
            .next()
            .expect("stored key should exist");
        assert!(marked.oauth_invalid_at_unix_secs.is_some());
        assert_eq!(
            marked.oauth_invalid_reason.as_deref(),
            Some("[OAUTH_EXPIRED] invalid access token")
        );
    }

    #[tokio::test]
    async fn oauth_invalidation_marks_claude_code_unauthorized_without_body() {
        let state = claude_code_oauth_state();
        let mut plan = sample_codex_plan();
        plan.provider_name = Some("claude_code".to_string());
        plan.provider_api_format = "claude:messages".to_string();

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: None,
            },
            LocalExecutionEffect::OauthInvalidation(LocalOAuthInvalidationEffect {
                status_code: 401,
                response_text: None,
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
            Some("[OAUTH_EXPIRED] OAuth access token was rejected")
        );
    }

    #[tokio::test]
    async fn oauth_invalidation_marks_codex_key_expired() {
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
            Some("[OAUTH_EXPIRED] Codex Token 已过期")
        );
        assert_eq!(
            stored_key
                .status_snapshot
                .as_ref()
                .and_then(|value| value.get("oauth"))
                .and_then(|value| value.get("code"))
                .and_then(Value::as_str),
            Some("expired")
        );
    }

    #[tokio::test]
    async fn oauth_success_clears_recoverable_codex_invalid_state() {
        let mut key = sample_codex_key();
        key.oauth_invalid_at_unix_secs = Some(100);
        key.oauth_invalid_reason = Some("[OAUTH_EXPIRED] session expired".to_string());
        key.upstream_metadata = Some(json!({
            "codex": {
                "credential_generation": "credential-generation-current",
                "oauth_state_request_started_at_unix_ms": 100_000u64,
                "oauth_state_request_id": "00000001-86a0-7000-8000-000000000001"
            }
        }));
        let state = codex_state_with_provider_and_key(sample_codex_provider(), key);
        let plan = sample_codex_plan();
        let report_context = json!({
            "codex_credential_generation": "credential-generation-current",
            "provider_request_started_at_unix_ms": 200_000u64,
            "provider_request_order_id": "00000003-0d40-7000-8000-000000000001"
        });

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: Some(&report_context),
            },
            LocalExecutionEffect::OauthSuccess(LocalOAuthSuccessEffect {
                status_code: 200,
                request_started_at_unix_ms: Some(200_000),
                request_order_id: Some("00000003-0d40-7000-8000-000000000001"),
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
        assert_eq!(
            stored_key.upstream_metadata.as_ref().and_then(
                |metadata| metadata.pointer("/codex/oauth_state_request_started_at_unix_ms")
            ),
            Some(&json!(200_000u64))
        );
        assert_eq!(
            stored_key
                .upstream_metadata
                .as_ref()
                .and_then(|metadata| metadata.pointer("/codex/oauth_state_request_id")),
            Some(&json!("00000003-0d40-7000-8000-000000000001"))
        );
    }

    #[tokio::test]
    async fn same_millisecond_older_oauth_success_does_not_clear_newer_codex_invalidation() {
        let mut key = sample_codex_key();
        key.oauth_invalid_at_unix_secs = Some(300);
        key.oauth_invalid_reason = Some("[OAUTH_EXPIRED] session expired".to_string());
        key.upstream_metadata = Some(json!({
            "codex": {
                "credential_generation": "credential-generation-current",
                "oauth_state_request_started_at_unix_ms": 300_000u64,
                "oauth_state_request_id": "00000004-93e0-7000-8000-000000000002"
            }
        }));
        let state = codex_state_with_provider_and_key(sample_codex_provider(), key);
        let plan = sample_codex_plan();
        let report_context = json!({
            "codex_credential_generation": "credential-generation-current",
            "provider_request_started_at_unix_ms": 300_000u64,
            "provider_request_order_id": "00000004-93e0-7000-8000-000000000001"
        });

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: Some(&report_context),
            },
            LocalExecutionEffect::OauthSuccess(LocalOAuthSuccessEffect {
                status_code: 200,
                request_started_at_unix_ms: Some(300_000),
                request_order_id: Some("00000004-93e0-7000-8000-000000000001"),
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
        assert_eq!(stored_key.oauth_invalid_at_unix_secs, Some(300));
        assert_eq!(
            stored_key.oauth_invalid_reason.as_deref(),
            Some("[OAUTH_EXPIRED] session expired")
        );
        assert_eq!(
            stored_key
                .upstream_metadata
                .as_ref()
                .and_then(|metadata| metadata.pointer("/codex/oauth_state_request_id")),
            Some(&json!("00000004-93e0-7000-8000-000000000002"))
        );
    }

    #[tokio::test]
    async fn oauth_success_preserves_codex_account_block() {
        let mut key = sample_codex_key();
        key.oauth_invalid_at_unix_secs = Some(100);
        key.oauth_invalid_reason = Some("[ACCOUNT_BLOCK] account deactivated".to_string());
        key.upstream_metadata = Some(json!({
            "codex": {
                "credential_generation": "credential-generation-current"
            }
        }));
        let state = codex_state_with_provider_and_key(sample_codex_provider(), key);
        let plan = sample_codex_plan();
        let report_context = json!({
            "codex_credential_generation": "credential-generation-current",
            "provider_request_started_at_unix_ms": 200_000u64,
            "provider_request_order_id": "00000003-0d40-7000-8000-000000000001"
        });

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: Some(&report_context),
            },
            LocalExecutionEffect::OauthSuccess(LocalOAuthSuccessEffect {
                status_code: 200,
                request_started_at_unix_ms: Some(200_000),
                request_order_id: Some("00000003-0d40-7000-8000-000000000001"),
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
        assert_eq!(stored_key.oauth_invalid_at_unix_secs, Some(100));
        assert_eq!(
            stored_key.oauth_invalid_reason.as_deref(),
            Some("[ACCOUNT_BLOCK] account deactivated")
        );
        assert_eq!(
            stored_key.upstream_metadata.as_ref().and_then(
                |metadata| metadata.pointer("/codex/oauth_state_request_started_at_unix_ms")
            ),
            Some(&json!(200_000u64))
        );
    }

    #[tokio::test]
    async fn oauth_success_does_not_clear_codex_invalid_state_for_replaced_bearer() {
        let mut key = sample_codex_key();
        key.oauth_invalid_at_unix_secs = Some(100);
        key.oauth_invalid_reason = Some("[OAUTH_EXPIRED] session expired".to_string());
        key.upstream_metadata = Some(json!({
            "codex": {
                "credential_generation": "credential-generation-current"
            }
        }));
        let state = codex_state_with_provider_and_key(sample_codex_provider(), key);
        let mut plan = sample_codex_plan();
        plan.headers.insert(
            "authorization".to_string(),
            "Bearer replaced-access-token".to_string(),
        );
        let report_context = json!({
            "codex_credential_generation": "credential-generation-current",
            "provider_request_started_at_unix_ms": 200_000u64,
            "provider_request_order_id": "00000003-0d40-7000-8000-000000000001"
        });

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: Some(&report_context),
            },
            LocalExecutionEffect::OauthSuccess(LocalOAuthSuccessEffect {
                status_code: 200,
                request_started_at_unix_ms: Some(200_000),
                request_order_id: Some("00000003-0d40-7000-8000-000000000001"),
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
        assert_eq!(stored_key.oauth_invalid_at_unix_secs, Some(100));
        assert_eq!(
            stored_key.oauth_invalid_reason.as_deref(),
            Some("[OAUTH_EXPIRED] session expired")
        );
        assert!(stored_key
            .upstream_metadata
            .as_ref()
            .and_then(|metadata| metadata.pointer("/codex/oauth_state_request_started_at_unix_ms"))
            .is_none());
    }

    #[tokio::test]
    async fn oauth_success_effect_ignores_non_success_status() {
        let mut key = sample_codex_key();
        key.oauth_invalid_at_unix_secs = Some(100);
        key.oauth_invalid_reason = Some("[OAUTH_EXPIRED] session expired".to_string());
        key.upstream_metadata = Some(json!({
            "codex": {
                "credential_generation": "credential-generation-current"
            }
        }));
        let state = codex_state_with_provider_and_key(sample_codex_provider(), key);
        let plan = sample_codex_plan();
        let report_context = json!({
            "codex_credential_generation": "credential-generation-current",
            "provider_request_started_at_unix_ms": 200_000u64,
            "provider_request_order_id": "00000003-0d40-7000-8000-000000000001"
        });

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: Some(&report_context),
            },
            LocalExecutionEffect::OauthSuccess(LocalOAuthSuccessEffect {
                status_code: 304,
                request_started_at_unix_ms: Some(200_000),
                request_order_id: Some("00000003-0d40-7000-8000-000000000001"),
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
        assert_eq!(stored_key.oauth_invalid_at_unix_secs, Some(100));
        assert_eq!(
            stored_key.oauth_invalid_reason.as_deref(),
            Some("[OAUTH_EXPIRED] session expired")
        );
        assert!(stored_key
            .upstream_metadata
            .as_ref()
            .and_then(|metadata| metadata.pointer("/codex/oauth_state_request_started_at_unix_ms"))
            .is_none());
    }

    #[tokio::test]
    async fn oauth_invalidation_marks_generic_codex_403_as_token_invalid() {
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
        assert!(stored_key.oauth_invalid_at_unix_secs.is_some());
        assert_eq!(
            stored_key.oauth_invalid_reason.as_deref(),
            Some("[OAUTH_EXPIRED] Codex Token 已失效 (403)")
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
    async fn oauth_invalidation_marks_codex_inactive_pat_owner_403_as_token_invalid() {
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
                response_text: Some(
                    r#"{"error":{"code":"biscuit_baker_service_auth_credential_error_status","message":"Personal access token owner is inactive."},"status":403}"#,
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
            Some("[OAUTH_EXPIRED] Codex Token 已失效")
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
    async fn oauth_invalidation_auto_remove_keeps_inactive_pat_owner() {
        let state = codex_state_with_auto_remove();
        let plan = sample_codex_plan();

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: None,
            },
            LocalExecutionEffect::OauthInvalidation(LocalOAuthInvalidationEffect {
                status_code: 403,
                response_text: Some(
                    r#"{"error":{"code":"biscuit_baker_service_auth_credential_error_status","message":"Personal access token owner is inactive."},"status":403}"#,
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
            .expect("recoverable token invalidation should retain the key");
        assert_eq!(
            stored_key.oauth_invalid_reason.as_deref(),
            Some("[OAUTH_EXPIRED] Codex Token 已失效")
        );
    }

    #[tokio::test]
    async fn oauth_invalidation_auto_removes_account_block() {
        let state = codex_state_with_auto_remove();
        let plan = sample_codex_plan();

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: None,
            },
            LocalExecutionEffect::OauthInvalidation(LocalOAuthInvalidationEffect {
                status_code: 403,
                response_text: Some(
                    r#"{"error":{"message":"account has been deactivated"},"status":403}"#,
                ),
            }),
        )
        .await;

        let stored_keys = state
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&plan.key_id))
            .await
            .expect("provider catalog keys should load");
        assert!(stored_keys.is_empty());
    }

    #[tokio::test]
    async fn oauth_invalidation_auto_remove_keeps_recoverable_expired_token() {
        let state = codex_state_with_auto_remove();
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
            .expect("recoverable expired token should be retained");
        assert_eq!(
            stored_key.oauth_invalid_reason.as_deref(),
            Some("[OAUTH_EXPIRED] Codex Token 已过期")
        );
    }

    #[tokio::test]
    async fn oauth_invalidation_does_not_mutate_replacement_after_agent_request() {
        let state = codex_state_with_auto_remove();
        let plan = sample_codex_agent_identity_plan();

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: None,
            },
            LocalExecutionEffect::OauthInvalidation(LocalOAuthInvalidationEffect {
                status_code: 403,
                response_text: Some(
                    r#"{"error":{"code":"biscuit_baker_service_auth_credential_error_status","message":"Personal access token owner is inactive."},"status":403}"#,
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
            .expect("replacement OAuth key should not be removed");
        assert_eq!(stored_key.oauth_invalid_at_unix_secs, None);
        assert_eq!(stored_key.oauth_invalid_reason, None);
    }

    #[tokio::test]
    async fn oauth_invalidation_does_not_mutate_agent_replacement_after_bearer_request() {
        let state = codex_state_with_provider_and_key(
            sample_codex_provider_with_auto_remove(),
            sample_codex_agent_identity_key(),
        );
        let mut plan = sample_codex_plan();
        plan.headers.insert(
            "authorization".to_string(),
            "Bearer old-access-token".to_string(),
        );

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: None,
            },
            LocalExecutionEffect::OauthInvalidation(LocalOAuthInvalidationEffect {
                status_code: 403,
                response_text: Some(
                    r#"{"error":{"code":"biscuit_baker_service_auth_credential_error_status","message":"Personal access token owner is inactive."},"status":403}"#,
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
            .expect("Agent Identity replacement should not be removed");
        assert_eq!(stored_key.oauth_invalid_at_unix_secs, None);
        assert_eq!(stored_key.oauth_invalid_reason, None);
    }

    #[tokio::test]
    async fn oauth_invalidation_rejects_stale_codex_credential_generation() {
        let mut key = sample_codex_key();
        key.upstream_metadata = Some(json!({
            "codex": {
                "credential_generation": "credential-generation-current"
            }
        }));
        let state = codex_state_with_provider_and_key(sample_codex_provider(), key);
        let plan = sample_codex_plan();
        let report_context = json!({
            "codex_credential_generation": "credential-generation-stale",
            "provider_request_started_at_unix_ms": 100_000u64,
            "provider_request_order_id": "00000001-86a0-7000-8000-000000000001"
        });

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: Some(&report_context),
            },
            LocalExecutionEffect::OauthInvalidation(LocalOAuthInvalidationEffect {
                status_code: 401,
                response_text: Some(r#"{"error":{"message":"session expired"}}"#),
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
        assert!(stored_key
            .upstream_metadata
            .as_ref()
            .and_then(|metadata| metadata.pointer("/codex/oauth_state_request_started_at_unix_ms"))
            .is_none());
    }

    #[tokio::test]
    async fn delayed_codex_quota_report_does_not_clear_newer_oauth_invalidation() {
        let mut key = sample_codex_key();
        key.upstream_metadata = Some(json!({
            "codex": {
                "credential_generation": "credential-generation-current",
                "account_quota_reset_generation": 0
            }
        }));
        let state = codex_state_with_provider_and_key(sample_codex_provider(), key);
        let plan = sample_codex_plan();
        let older_request_id = "00000001-86a0-7000-8000-000000000001";
        let newer_request_id = "00000001-86a0-7000-8000-000000000002";
        let older_uuid = uuid::Uuid::parse_str(older_request_id).expect("older id should parse");
        let newer_uuid = uuid::Uuid::parse_str(newer_request_id).expect("newer id should parse");
        assert_eq!(older_uuid.get_version_num(), 7);
        assert_eq!(newer_uuid.get_version_num(), 7);
        assert!(older_uuid < newer_uuid);

        let invalidation_context = json!({
            "codex_credential_generation": "credential-generation-current",
            "provider_request_started_at_unix_ms": 100_000u64,
            "provider_request_order_id": newer_request_id
        });
        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: Some(&invalidation_context),
            },
            LocalExecutionEffect::OauthInvalidation(LocalOAuthInvalidationEffect {
                status_code: 401,
                response_text: Some(r#"{"error":{"message":"session expired"}}"#),
            }),
        )
        .await;

        let delayed_report_context = json!({
            "key_id": plan.key_id,
            "codex_credential_generation": "credential-generation-current",
            "codex_quota_reset_generation": 0,
            "provider_request_started_at_unix_ms": 100_000u64,
            "provider_response_headers_observed_at_unix_ms": 110_000u64,
            "provider_request_order_id": older_request_id
        });
        let delayed_headers = BTreeMap::from([
            ("x-codex-plan-type".to_string(), "free".to_string()),
            ("x-codex-primary-used-percent".to_string(), "25".to_string()),
            (
                "x-codex-primary-reset-at".to_string(),
                "2000000000".to_string(),
            ),
            (
                "x-codex-primary-window-minutes".to_string(),
                "300".to_string(),
            ),
        ]);
        let delayed_report = GatewaySyncReportRequest {
            trace_id: "trace-delayed-codex-quota".to_string(),
            report_kind: "openai_responses_sync_success".to_string(),
            report_context: Some(delayed_report_context),
            status_code: 200,
            headers: delayed_headers,
            body_json: None,
            client_body_json: None,
            body_base64: None,
            telemetry: None,
        };
        apply_local_report_effect(
            &state,
            LocalReportEffect::Sync {
                payload: &delayed_report,
            },
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
            stored_key.oauth_invalid_reason.as_deref(),
            Some("[OAUTH_EXPIRED] Codex Token 已过期")
        );
        assert!(stored_key.oauth_invalid_at_unix_secs.is_some());
        assert_eq!(
            stored_key
                .upstream_metadata
                .as_ref()
                .and_then(|metadata| metadata.pointer("/codex/oauth_state_request_id")),
            Some(&json!(newer_request_id))
        );
        assert_eq!(
            stored_key
                .status_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.pointer("/oauth/code")),
            Some(&json!("expired"))
        );
    }

    #[tokio::test]
    async fn health_failure_updates_codex_key_for_current_bearer_request() {
        let state = codex_state();
        let plan = sample_codex_plan();

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
            stored_key
                .health_by_format
                .as_ref()
                .and_then(|value| value.get("openai:responses"))
                .and_then(|value| value.get("consecutive_failures"))
                .and_then(Value::as_u64),
            Some(1)
        );
    }

    #[tokio::test]
    async fn health_failure_does_not_mutate_codex_bearer_replacement() {
        let mut replacement = sample_codex_key();
        replacement.encrypted_api_key = Some(
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "replacement-token")
                .expect("replacement token should encrypt"),
        );
        replacement.encrypted_auth_config = Some(
            encrypt_python_fernet_plaintext(
                DEVELOPMENT_ENCRYPTION_KEY,
                r#"{"provider_type":"codex","refresh_token":"replacement-refresh-token"}"#,
            )
            .expect("replacement auth config should encrypt"),
        );
        let expected_health = replacement.health_by_format.clone();
        let expected_circuit = replacement.circuit_breaker_by_format.clone();
        let state = codex_state_with_provider_and_key(sample_codex_provider(), replacement);
        let plan = sample_codex_plan();

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
            .expect("replacement key should exist");
        assert_eq!(stored_key.health_by_format, expected_health);
        assert_eq!(stored_key.circuit_breaker_by_format, expected_circuit);
    }

    #[tokio::test]
    async fn adaptive_rate_limit_does_not_mutate_codex_bearer_replacement() {
        let mut replacement = sample_codex_key();
        replacement.encrypted_api_key = Some(
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "replacement-token")
                .expect("replacement token should encrypt"),
        );
        replacement.encrypted_auth_config = Some(
            encrypt_python_fernet_plaintext(
                DEVELOPMENT_ENCRYPTION_KEY,
                r#"{"provider_type":"codex","refresh_token":"replacement-refresh-token"}"#,
            )
            .expect("replacement auth config should encrypt"),
        );
        replacement.learned_rpm_limit = Some(12);
        replacement.rpm_429_count = Some(1);
        let expected_adaptive_state = ProviderCatalogKeyAdaptiveState::from(&replacement);
        let state = codex_state_with_provider_and_key(sample_codex_provider(), replacement);
        let plan = sample_codex_plan();

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
            .expect("replacement key should exist");
        assert_eq!(
            ProviderCatalogKeyAdaptiveState::from(&stored_key),
            expected_adaptive_state
        );
    }

    #[tokio::test]
    async fn pool_error_does_not_clear_codex_bearer_replacement_circuit() {
        let legacy_circuit = json!({
            "openai:responses": {
                "open": true,
                "reason": "replacement-state"
            }
        });
        let mut replacement = sample_codex_key();
        replacement.encrypted_api_key = Some(
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "replacement-token")
                .expect("replacement token should encrypt"),
        );
        replacement.encrypted_auth_config = Some(
            encrypt_python_fernet_plaintext(
                DEVELOPMENT_ENCRYPTION_KEY,
                r#"{"provider_type":"codex","refresh_token":"replacement-refresh-token"}"#,
            )
            .expect("replacement auth config should encrypt"),
        );
        replacement.circuit_breaker_by_format = Some(legacy_circuit.clone());
        let state = codex_state_with_provider_and_key(sample_codex_provider(), replacement);
        let plan = sample_codex_plan();

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
            .expect("replacement key should exist");
        assert_eq!(stored_key.circuit_breaker_by_format, Some(legacy_circuit));
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
    async fn runtime_health_failure_does_not_reactivate_admin_disabled_key() {
        let mut key = sample_health_key();
        key.is_active = false;
        let state = health_state_with_key(key);
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
        assert!(!stored_key.is_active);
        assert_eq!(
            stored_key
                .health_by_format
                .as_ref()
                .and_then(|value| value.get("openai:chat"))
                .and_then(|value| value.get("consecutive_failures"))
                .and_then(Value::as_u64),
            Some(1)
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
    async fn concurrent_health_failures_for_one_key_do_not_lose_updates() {
        let state = health_state();
        let plan = sample_plan();
        let mut tasks = tokio::task::JoinSet::new();

        for _ in 0..8 {
            let state = state.clone();
            let plan = plan.clone();
            tasks.spawn(async move {
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
            });
        }
        while let Some(result) = tasks.join_next().await {
            result.expect("health failure task should complete");
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
        assert_eq!(
            stored_key
                .health_by_format
                .as_ref()
                .and_then(|value| value.get("openai:chat"))
                .and_then(|value| value.get("consecutive_failures"))
                .and_then(Value::as_u64),
            Some(8)
        );
        assert_eq!(circuit["open"], json!(true));
        assert_eq!(circuit["reason"], json!("consecutive_failures_8"));
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
    async fn health_success_projection_is_rate_limited_until_failure_resets_gate() {
        // Keep this test's process-wide persistence gate isolated from the other
        // effect tests, which intentionally exercise the same health key in parallel.
        let mut plan = sample_plan();
        plan.key_id = format!("health-success-rate-limit-{}", uuid::Uuid::new_v4());
        let mut key = sample_health_key();
        key.id = plan.key_id.clone();
        let state = health_state_with_key(key);

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: None,
            },
            LocalExecutionEffect::HealthSuccess(LocalHealthSuccessEffect),
        )
        .await;
        let first_updated_at = state
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&plan.key_id))
            .await
            .expect("provider catalog keys should load")
            .into_iter()
            .next()
            .expect("stored key should exist")
            .updated_at_unix_secs;

        apply_local_execution_effect(
            &state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: None,
            },
            LocalExecutionEffect::HealthSuccess(LocalHealthSuccessEffect),
        )
        .await;
        let second_updated_at = state
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&plan.key_id))
            .await
            .expect("provider catalog keys should load")
            .into_iter()
            .next()
            .expect("stored key should exist")
            .updated_at_unix_secs;
        assert_eq!(second_updated_at, first_updated_at);

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
            stored_key
                .health_by_format
                .as_ref()
                .and_then(|value| value.get("openai:chat"))
                .and_then(|value| value.get("consecutive_failures"))
                .and_then(Value::as_u64),
            Some(0)
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
    async fn adaptive_rate_limit_effect_preserves_quota_status_owned_by_reports() {
        let mut key = sample_adaptive_key();
        key.status_snapshot = Some(json!({
            "quota": {"remaining": 9},
            "oauth": {"invalid": false},
            "observation_count": 0
        }));
        let state = health_state_with_key(key);
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
        let status = stored_key.status_snapshot.expect("status should exist");
        assert_eq!(status["quota"], json!({"remaining":9}));
        assert_eq!(status["oauth"], json!({"invalid":false}));
        assert_eq!(status["observation_count"], json!(1));
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
