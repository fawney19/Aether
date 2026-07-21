use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};

use aether_data::repository::users::StoredUserGroup;
use aether_data_contracts::repository::billing::UserDailyQuotaAvailabilityRecord;
use aether_data_contracts::repository::quota::StoredProviderQuotaSnapshot;
use aether_data_contracts::repository::usage::UsageCounterHealthSnapshot;
use aether_runtime::ConcurrencyGate;
use aether_runtime_state::{RuntimeSemaphore, RuntimeState};
use dashmap::DashMap;
use tokio::sync::{Mutex as TokioMutex, RwLock as TokioRwLock, Semaphore};

use super::super::async_task::{VideoTaskPollerConfig, VideoTaskService};
use super::super::cache::{
    AuthApiKeyFeatureCacheKey, AuthApiKeyIdentityCacheKey, AuthApiKeyLastUsedCache,
    AuthContextCache, AuthSnapshotCache, DashboardResponseCache, DirectPlanBypassCache,
    JsonValueCache, SchedulerAffinityCache, SystemConfigCache, ValueCache,
};
use super::super::data::GatewayDataState;
use super::super::fallback_metrics;
use super::super::maintenance::UsageCounterFlushRuntimeMetrics;
use super::super::rate_limit::FrontdoorUserRpmLimiter;
use super::super::request_candidate_queue::RequestCandidateQueueRuntime;
use super::super::task_runtime::TaskSupervisorMetrics;
use super::super::{provider_transport, usage};
use super::{
    AdminBillingCollectorRecord, AdminBillingRuleRecord, AdminPaymentCallbackRecord,
    AdminWalletPaymentOrderRecord, AdminWalletRefundRecord, AdminWalletTransactionRecord,
    CachedProviderTransportSnapshot, FrontdoorCorsConfig, LocalExecutionRuntimeMissDiagnostic,
    LocalProviderDeleteTaskState, ProviderTransportSnapshotCacheKey,
};

const DEFAULT_REQUEST_BODY_READ_TIMEOUT_MS: u64 = 120_000;
const MIN_REQUEST_BODY_READ_TIMEOUT_MS: u64 = 1_000;
const MAX_REQUEST_BODY_READ_TIMEOUT_MS: u64 = 600_000;
const REQUEST_BODY_READ_TIMEOUT_MS_ENV: &str = "AETHER_GATEWAY_REQUEST_BODY_READ_TIMEOUT_MS";
const DEFAULT_REQUEST_BODY_BUFFER_BUDGET_MB: usize = 256;
const MIN_REQUEST_BODY_BUFFER_BUDGET_MB: usize = 64;
const MAX_REQUEST_BODY_BUFFER_BUDGET_MB: usize = 16 * 1024;
const REQUEST_BODY_BUFFER_BUDGET_MB_ENV: &str = "AETHER_GATEWAY_REQUEST_BODY_BUFFER_BUDGET_MB";
pub(crate) const REQUEST_BODY_BUFFER_PERMIT_BYTES: usize = 64 * 1024;

const DEFAULT_LOCAL_EXECUTION_PLANNING_TIMEOUT_MS: u64 = 30_000;
const MIN_LOCAL_EXECUTION_PLANNING_TIMEOUT_MS: u64 = 500;
const MAX_LOCAL_EXECUTION_PLANNING_TIMEOUT_MS: u64 = 120_000;
const LOCAL_EXECUTION_PLANNING_TIMEOUT_MS_ENV: &str =
    "AETHER_GATEWAY_LOCAL_EXECUTION_PLANNING_TIMEOUT_MS";
const DEFAULT_AUTH_SNAPSHOT_LOAD_GATE_LIMIT: usize = 64;
const DEFAULT_CANDIDATE_PLANNING_GATE_LIMIT: usize = 1024;
const DEFAULT_UPSTREAM_EXECUTION_GATE_LIMIT: usize = 10_000;
const DEFAULT_UPSTREAM_TARGET_GATE_LIMIT: usize = 10_000;
const MAX_AUTH_SNAPSHOT_LOAD_GATE_LIMIT: usize = 1024;
const MAX_CANDIDATE_PLANNING_GATE_LIMIT: usize = 8192;
const MAX_UPSTREAM_EXECUTION_GATE_LIMIT: usize = 16_384;
const MAX_UPSTREAM_TARGET_GATE_LIMIT: usize = 16_384;
const AUTH_SNAPSHOT_LOAD_GATE_LIMIT_PER_CPU: usize = 16;
const CANDIDATE_PLANNING_GATE_LIMIT_PER_CPU: usize = 256;
const UPSTREAM_EXECUTION_GATE_LIMIT_PER_CPU: usize = 1024;
const UPSTREAM_TARGET_GATE_LIMIT_PER_CPU: usize = 1024;
const GATE_LIMIT_FD_RESERVE: usize = 128;
const DEFAULT_INTERNAL_GATE_QUEUE_BUDGET_MS: u64 = 250;
const MAX_INTERNAL_GATE_QUEUE_BUDGET_MS: u64 = 5_000;
const AUTH_SNAPSHOT_LOAD_GATE_LIMIT_ENV: &str = "AETHER_GATEWAY_AUTH_SNAPSHOT_LOAD_GATE_LIMIT";
const CANDIDATE_PLANNING_GATE_LIMIT_ENV: &str = "AETHER_GATEWAY_CANDIDATE_PLANNING_GATE_LIMIT";
const UPSTREAM_EXECUTION_GATE_LIMIT_ENV: &str = "AETHER_GATEWAY_UPSTREAM_EXECUTION_GATE_LIMIT";
const UPSTREAM_TARGET_GATE_LIMIT_ENV: &str = "AETHER_GATEWAY_UPSTREAM_TARGET_GATE_LIMIT";
const INTERNAL_GATE_QUEUE_BUDGET_MS_ENV: &str = "AETHER_GATEWAY_INTERNAL_GATE_QUEUE_BUDGET_MS";
const DEFAULT_AUTH_CAPACITY_CACHE_TTL_MS: u64 = 500;
const MIN_AUTH_CAPACITY_CACHE_TTL_MS: u64 = 10;
const MAX_AUTH_CAPACITY_CACHE_TTL_MS: u64 = 10_000;
const AUTH_CAPACITY_CACHE_TTL_MS_ENV: &str = "AETHER_GATEWAY_AUTH_CAPACITY_CACHE_TTL_MS";

#[cfg(test)]
type TestExecutionRuntimeSyncOverrideFn = dyn Fn(
        &aether_contracts::ExecutionPlan,
    ) -> Result<aether_contracts::ExecutionResult, crate::GatewayError>
    + Send
    + Sync;

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct TestExecutionRuntimeSyncOverride(
    pub(crate) Arc<TestExecutionRuntimeSyncOverrideFn>,
);

#[cfg(test)]
impl std::fmt::Debug for TestExecutionRuntimeSyncOverride {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TestExecutionRuntimeSyncOverride(..)")
    }
}

#[derive(Debug, Clone)]
pub(crate) struct FrontdoorRuntimeGuardConfig {
    pub(crate) request_body_read_timeout: Duration,
    pub(crate) request_body_buffer_budget_bytes: usize,
    pub(crate) request_body_buffer_budget_permits: usize,
    pub(crate) local_execution_planning_timeout: Duration,
    pub(crate) internal_gate_queue_budget: Duration,
    pub(crate) auth_capacity_cache_ttl: Duration,
    pub(crate) auth_snapshot_load_gate_limit: Option<usize>,
    pub(crate) candidate_planning_gate_limit: Option<usize>,
    pub(crate) upstream_execution_gate_limit: Option<usize>,
    pub(crate) upstream_target_gate_limit: Option<usize>,
}

pub(crate) const METRIC_SNAPSHOT_TTL: Duration = Duration::from_secs(2);

impl FrontdoorRuntimeGuardConfig {
    pub(crate) fn from_env() -> Self {
        Self {
            request_body_read_timeout: env_duration_ms(
                REQUEST_BODY_READ_TIMEOUT_MS_ENV,
                DEFAULT_REQUEST_BODY_READ_TIMEOUT_MS,
                MIN_REQUEST_BODY_READ_TIMEOUT_MS,
                MAX_REQUEST_BODY_READ_TIMEOUT_MS,
            ),
            request_body_buffer_budget_bytes: request_body_buffer_budget_bytes_from_env(),
            request_body_buffer_budget_permits: request_body_buffer_budget_permits_from_env(),
            local_execution_planning_timeout: env_duration_ms(
                LOCAL_EXECUTION_PLANNING_TIMEOUT_MS_ENV,
                DEFAULT_LOCAL_EXECUTION_PLANNING_TIMEOUT_MS,
                MIN_LOCAL_EXECUTION_PLANNING_TIMEOUT_MS,
                MAX_LOCAL_EXECUTION_PLANNING_TIMEOUT_MS,
            ),
            internal_gate_queue_budget: env_duration_ms(
                INTERNAL_GATE_QUEUE_BUDGET_MS_ENV,
                DEFAULT_INTERNAL_GATE_QUEUE_BUDGET_MS,
                1,
                MAX_INTERNAL_GATE_QUEUE_BUDGET_MS,
            ),
            auth_capacity_cache_ttl: env_cache_duration_ms(
                AUTH_CAPACITY_CACHE_TTL_MS_ENV,
                DEFAULT_AUTH_CAPACITY_CACHE_TTL_MS,
                MIN_AUTH_CAPACITY_CACHE_TTL_MS,
                MAX_AUTH_CAPACITY_CACHE_TTL_MS,
            ),
            auth_snapshot_load_gate_limit: auth_snapshot_load_gate_limit_from_env(),
            candidate_planning_gate_limit: candidate_planning_gate_limit_from_env(),
            upstream_execution_gate_limit: upstream_execution_gate_limit_from_env(),
            upstream_target_gate_limit: upstream_target_gate_limit_from_env(),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_tests(
        request_body_read_timeout: Duration,
        local_execution_planning_timeout: Duration,
    ) -> Self {
        Self {
            request_body_read_timeout,
            request_body_buffer_budget_bytes: DEFAULT_REQUEST_BODY_BUFFER_BUDGET_MB * 1024 * 1024,
            request_body_buffer_budget_permits: DEFAULT_REQUEST_BODY_BUFFER_BUDGET_MB * 16,
            local_execution_planning_timeout,
            internal_gate_queue_budget: Duration::from_millis(
                DEFAULT_INTERNAL_GATE_QUEUE_BUDGET_MS,
            ),
            auth_capacity_cache_ttl: Duration::from_millis(DEFAULT_AUTH_CAPACITY_CACHE_TTL_MS),
            auth_snapshot_load_gate_limit: Some(DEFAULT_AUTH_SNAPSHOT_LOAD_GATE_LIMIT),
            candidate_planning_gate_limit: Some(DEFAULT_CANDIDATE_PLANNING_GATE_LIMIT),
            upstream_execution_gate_limit: Some(DEFAULT_UPSTREAM_EXECUTION_GATE_LIMIT),
            upstream_target_gate_limit: Some(DEFAULT_UPSTREAM_TARGET_GATE_LIMIT),
        }
    }
}

fn request_body_buffer_budget_mb_from_env() -> usize {
    std::env::var(REQUEST_BODY_BUFFER_BUDGET_MB_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_REQUEST_BODY_BUFFER_BUDGET_MB)
        .clamp(
            MIN_REQUEST_BODY_BUFFER_BUDGET_MB,
            MAX_REQUEST_BODY_BUFFER_BUDGET_MB,
        )
}

fn request_body_buffer_budget_bytes_from_env() -> usize {
    request_body_buffer_budget_mb_from_env().saturating_mul(1024 * 1024)
}

fn request_body_buffer_budget_permits_from_env() -> usize {
    request_body_buffer_budget_bytes_from_env().saturating_add(REQUEST_BODY_BUFFER_PERMIT_BYTES - 1)
        / REQUEST_BODY_BUFFER_PERMIT_BYTES
}

fn env_duration_ms(key: &str, default_ms: u64, min_ms: u64, max_ms: u64) -> Duration {
    let ms = std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default_ms)
        .clamp(min_ms, max_ms);
    Duration::from_millis(ms)
}

fn env_cache_duration_ms(key: &str, default_ms: u64, min_ms: u64, max_ms: u64) -> Duration {
    let Some(raw) = std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    else {
        return Duration::from_millis(default_ms);
    };
    let Some(parsed) = raw.parse::<u64>().ok() else {
        return Duration::from_millis(default_ms);
    };
    if parsed == 0 {
        return Duration::ZERO;
    }
    Duration::from_millis(parsed.clamp(min_ms, max_ms))
}

#[derive(Debug, Clone, Copy)]
struct GateAutoProfile {
    floor: usize,
    cap: usize,
    per_cpu: usize,
    fd_divisor: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
struct GateAutoCapacity {
    cpu_parallelism: usize,
    fd_soft_limit: usize,
}

const CANDIDATE_PLANNING_GATE_AUTO_PROFILE: GateAutoProfile = GateAutoProfile {
    floor: DEFAULT_CANDIDATE_PLANNING_GATE_LIMIT,
    cap: MAX_CANDIDATE_PLANNING_GATE_LIMIT,
    per_cpu: CANDIDATE_PLANNING_GATE_LIMIT_PER_CPU,
    fd_divisor: None,
};

const AUTH_SNAPSHOT_LOAD_GATE_AUTO_PROFILE: GateAutoProfile = GateAutoProfile {
    floor: DEFAULT_AUTH_SNAPSHOT_LOAD_GATE_LIMIT,
    cap: MAX_AUTH_SNAPSHOT_LOAD_GATE_LIMIT,
    per_cpu: AUTH_SNAPSHOT_LOAD_GATE_LIMIT_PER_CPU,
    fd_divisor: None,
};

const UPSTREAM_EXECUTION_GATE_AUTO_PROFILE: GateAutoProfile = GateAutoProfile {
    floor: DEFAULT_UPSTREAM_EXECUTION_GATE_LIMIT,
    cap: MAX_UPSTREAM_EXECUTION_GATE_LIMIT,
    per_cpu: UPSTREAM_EXECUTION_GATE_LIMIT_PER_CPU,
    fd_divisor: Some(2),
};

const UPSTREAM_TARGET_GATE_AUTO_PROFILE: GateAutoProfile = GateAutoProfile {
    floor: DEFAULT_UPSTREAM_TARGET_GATE_LIMIT,
    cap: MAX_UPSTREAM_TARGET_GATE_LIMIT,
    per_cpu: UPSTREAM_TARGET_GATE_LIMIT_PER_CPU,
    fd_divisor: Some(4),
};

fn candidate_planning_gate_limit_from_env() -> Option<usize> {
    env_gate_limit(
        CANDIDATE_PLANNING_GATE_LIMIT_ENV,
        CANDIDATE_PLANNING_GATE_AUTO_PROFILE,
    )
}

fn auth_snapshot_load_gate_limit_from_env() -> Option<usize> {
    env_gate_limit(
        AUTH_SNAPSHOT_LOAD_GATE_LIMIT_ENV,
        AUTH_SNAPSHOT_LOAD_GATE_AUTO_PROFILE,
    )
}

fn upstream_execution_gate_limit_from_env() -> Option<usize> {
    env_gate_limit(
        UPSTREAM_EXECUTION_GATE_LIMIT_ENV,
        UPSTREAM_EXECUTION_GATE_AUTO_PROFILE,
    )
}

pub(crate) fn upstream_target_gate_limit_from_env() -> Option<usize> {
    env_gate_limit(
        UPSTREAM_TARGET_GATE_LIMIT_ENV,
        UPSTREAM_TARGET_GATE_AUTO_PROFILE,
    )
}

pub(crate) fn upstream_target_gate_auto_limit() -> usize {
    auto_gate_limit(
        UPSTREAM_TARGET_GATE_AUTO_PROFILE,
        current_gate_auto_capacity(),
    )
}

fn env_gate_limit(key: &str, profile: GateAutoProfile) -> Option<usize> {
    let raw = std::env::var(key).ok();
    parse_gate_limit_value(raw.as_deref(), profile, current_gate_auto_capacity())
}

fn parse_gate_limit_value(
    raw: Option<&str>,
    profile: GateAutoProfile,
    capacity: GateAutoCapacity,
) -> Option<usize> {
    let Some(value) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Some(auto_gate_limit(profile, capacity));
    };
    let normalized = value.to_ascii_lowercase();
    match normalized.as_str() {
        "auto" => Some(auto_gate_limit(profile, capacity)),
        "off" | "none" | "disabled" | "disable" => None,
        _ => match value.parse::<usize>() {
            Ok(0) => None,
            Ok(limit) => Some(limit.max(1)),
            Err(_) => Some(auto_gate_limit(profile, capacity)),
        },
    }
}

fn auto_gate_limit(profile: GateAutoProfile, capacity: GateAutoCapacity) -> usize {
    let cpu_limit = capacity
        .cpu_parallelism
        .max(1)
        .saturating_mul(profile.per_cpu.max(1));
    let mut limit = cpu_limit.max(profile.floor).min(profile.cap);
    if let Some(fd_divisor) = profile.fd_divisor.filter(|value| *value > 0) {
        let fd_budget = capacity
            .fd_soft_limit
            .saturating_sub(GATE_LIMIT_FD_RESERVE)
            .checked_div(fd_divisor)
            .unwrap_or(1)
            .max(1);
        limit = limit.min(fd_budget);
    }
    limit.max(1)
}

fn current_gate_auto_capacity() -> GateAutoCapacity {
    GateAutoCapacity {
        cpu_parallelism: std::thread::available_parallelism()
            .map(|value| value.get())
            .unwrap_or(1)
            .max(1),
        fd_soft_limit: soft_fd_limit().unwrap_or(1024).max(1),
    }
}

fn soft_fd_limit() -> Option<usize> {
    #[cfg(unix)]
    {
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        let result = unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) };
        if result == 0 {
            return usize::try_from(limit.rlim_cur).ok();
        }
    }
    None
}

#[derive(Debug, Clone)]
pub struct AppState {
    #[cfg(test)]
    pub(crate) execution_runtime_override_base_url: Option<String>,
    #[cfg(test)]
    pub(crate) execution_runtime_sync_override: Option<TestExecutionRuntimeSyncOverride>,
    pub(crate) data: Arc<GatewayDataState>,
    pub(crate) background_data: Arc<GatewayDataState>,
    pub(crate) background_data_isolated: bool,
    pub(crate) runtime_state: Arc<RuntimeState>,
    pub(crate) usage_runtime: Arc<usage::UsageRuntime>,
    pub(crate) video_tasks: Arc<VideoTaskService>,
    pub(crate) video_task_poller: Option<VideoTaskPollerConfig>,
    pub(crate) frontdoor_runtime_guards: Arc<FrontdoorRuntimeGuardConfig>,
    pub(crate) request_body_buffer_budget: Arc<Semaphore>,
    pub(crate) request_gate: Option<Arc<ConcurrencyGate>>,
    pub(crate) auth_snapshot_load_gate: Option<Arc<ConcurrencyGate>>,
    pub(crate) candidate_planning_gate: Option<Arc<ConcurrencyGate>>,
    pub(crate) upstream_execution_gate: Option<Arc<ConcurrencyGate>>,
    pub(crate) upstream_target_admission: Arc<crate::upstream_admission::UpstreamTargetAdmission>,
    pub(crate) distributed_request_gate: Option<Arc<RuntimeSemaphore>>,
    pub(crate) client: reqwest::Client,
    pub(crate) owner_forward_client: reqwest::Client,
    pub(crate) auth_context_cache: Arc<AuthContextCache>,
    pub(crate) auth_snapshot_cache: Arc<AuthSnapshotCache>,
    pub(crate) admin_security_blacklist_cache: Arc<ValueCache<String, bool>>,
    pub(crate) admin_security_whitelist_cache: Arc<ValueCache<String, Vec<String>>>,
    pub(crate) user_model_capability_settings_cache: Arc<JsonValueCache<String>>,
    pub(crate) user_feature_settings_cache: Arc<JsonValueCache<String>>,
    pub(crate) auth_api_key_force_capabilities_cache:
        Arc<JsonValueCache<AuthApiKeyIdentityCacheKey>>,
    pub(crate) auth_api_key_feature_settings_cache: Arc<JsonValueCache<AuthApiKeyFeatureCacheKey>>,
    pub(crate) auth_daily_quota_availability_cache:
        Arc<ValueCache<String, UserDailyQuotaAvailabilityRecord>>,
    pub(crate) auth_wallet_snapshot_cache:
        Arc<ValueCache<String, aether_data::repository::wallet::StoredWalletSnapshot>>,
    pub(crate) auth_request_cost_upper_bound_cache: Arc<ValueCache<String, f64>>,
    pub(crate) provider_quota_snapshot_cache: Arc<ValueCache<String, StoredProviderQuotaSnapshot>>,
    pub(crate) user_groups_for_user_cache: Arc<ValueCache<String, Vec<StoredUserGroup>>>,
    pub(crate) routing_group_selection_cache:
        Arc<ValueCache<String, crate::routing::GatewayRoutingGroupSelection>>,
    pub(crate) auth_api_key_last_used_cache: Arc<AuthApiKeyLastUsedCache>,
    pub(crate) oauth_refresh: Arc<provider_transport::LocalOAuthRefreshCoordinator>,
    pub(crate) direct_plan_bypass_cache: Arc<DirectPlanBypassCache>,
    pub(crate) scheduler_affinity_cache: Arc<SchedulerAffinityCache>,
    pub(crate) scheduler_affinity_epoch: Arc<AtomicU64>,
    pub(crate) dashboard_response_cache: Arc<DashboardResponseCache>,
    pub(crate) system_config_cache: Arc<SystemConfigCache>,
    pub(crate) candidate_row_page_cache: Arc<super::super::cache::CandidateRowPageCache>,
    pub(crate) candidate_page_cache: Arc<super::super::cache::CandidatePageCache>,
    pub(crate) candidate_resolved_page_cache: Arc<super::super::cache::CandidateResolvedPageCache>,
    pub(crate) chat_pii_redaction_runtime_config_cache:
        crate::privacy::ChatPiiRedactionRuntimeConfigCacheHandle,
    pub(crate) fallback_metrics: Arc<fallback_metrics::GatewayFallbackMetrics>,
    pub(crate) usage_counter_flush_metrics: Arc<UsageCounterFlushRuntimeMetrics>,
    pub(crate) task_supervisor_metrics: TaskSupervisorMetrics,
    pub(crate) process_resource_monitor: Arc<crate::process_metrics::GatewayProcessResourceMonitor>,
    pub(crate) metric_snapshot:
        Arc<TokioRwLock<Option<(Instant, Vec<aether_runtime::MetricSample>)>>>,
    pub(crate) metric_snapshot_refresh: Arc<TokioMutex<()>>,
    pub(crate) usage_counter_exact_health_metric_snapshot:
        Arc<TokioRwLock<Option<(Instant, UsageCounterHealthSnapshot)>>>,
    pub(crate) usage_counter_exact_health_metric_last_attempt: Arc<StdMutex<Option<Instant>>>,
    pub(crate) usage_counter_exact_health_metric_refresh: Arc<TokioMutex<()>>,
    pub(crate) request_candidate_queue: Option<Arc<RequestCandidateQueueRuntime>>,
    pub(crate) frontdoor_cors: Option<Arc<FrontdoorCorsConfig>>,
    pub(crate) frontdoor_user_rpm: Arc<FrontdoorUserRpmLimiter>,
    pub(crate) tunnel: crate::tunnel::EmbeddedTunnelState,
    pub(crate) provider_transport_snapshot_cache:
        Arc<DashMap<ProviderTransportSnapshotCacheKey, CachedProviderTransportSnapshot>>,
    pub(crate) provider_transport_snapshot_inflight:
        Arc<DashMap<ProviderTransportSnapshotCacheKey, Arc<TokioMutex<()>>>>,
    pub(crate) provider_key_rpm_resets: Arc<StdMutex<HashMap<String, u64>>>,
    pub(crate) local_execution_runtime_miss_diagnostics:
        Arc<DashMap<String, LocalExecutionRuntimeMissDiagnostic>>,
    pub(crate) admin_monitoring_error_stats_reset_at: Arc<StdMutex<Option<u64>>>,
    pub(crate) provider_delete_tasks: Arc<StdMutex<HashMap<String, LocalProviderDeleteTaskState>>>,
    #[cfg(test)]
    pub(crate) turnstile_siteverify_url_override: Option<String>,
    #[cfg(test)]
    pub(crate) turnstile_siteverify_timeout_override: Option<Duration>,
    #[cfg(test)]
    pub(crate) provider_oauth_state_store: Option<Arc<StdMutex<HashMap<String, String>>>>,
    #[cfg(test)]
    pub(crate) provider_oauth_device_session_store: Option<Arc<StdMutex<HashMap<String, String>>>>,
    #[cfg(test)]
    pub(crate) provider_oauth_batch_task_store: Option<Arc<StdMutex<HashMap<String, String>>>>,
    #[cfg(test)]
    pub(crate) auth_session_store:
        Option<Arc<StdMutex<HashMap<String, crate::data::state::StoredUserSessionRecord>>>>,
    #[cfg(test)]
    pub(crate) auth_email_verification_store: Option<Arc<StdMutex<HashMap<String, String>>>>,
    #[cfg(test)]
    pub(crate) auth_email_delivery_store: Option<Arc<StdMutex<Vec<serde_json::Value>>>>,
    #[cfg(test)]
    pub(crate) auth_user_store: Option<
        Arc<StdMutex<HashMap<String, aether_data::repository::users::StoredUserAuthRecord>>>,
    >,
    #[cfg(test)]
    pub(crate) auth_user_model_capability_store:
        Option<Arc<StdMutex<HashMap<String, serde_json::Value>>>>,
    #[cfg(test)]
    pub(crate) auth_wallet_store: Option<
        Arc<StdMutex<HashMap<String, aether_data::repository::wallet::StoredWalletSnapshot>>>,
    >,
    #[cfg(test)]
    pub(crate) admin_wallet_payment_order_store:
        Option<Arc<StdMutex<HashMap<String, AdminWalletPaymentOrderRecord>>>>,
    #[cfg(test)]
    pub(crate) admin_payment_callback_store:
        Option<Arc<StdMutex<HashMap<String, AdminPaymentCallbackRecord>>>>,
    #[cfg(test)]
    pub(crate) admin_wallet_transaction_store:
        Option<Arc<StdMutex<HashMap<String, AdminWalletTransactionRecord>>>>,
    #[cfg(test)]
    pub(crate) admin_wallet_refund_store:
        Option<Arc<StdMutex<HashMap<String, AdminWalletRefundRecord>>>>,
    #[cfg(test)]
    pub(crate) admin_billing_rule_store:
        Option<Arc<StdMutex<HashMap<String, AdminBillingRuleRecord>>>>,
    #[cfg(test)]
    pub(crate) admin_billing_collector_store:
        Option<Arc<StdMutex<HashMap<String, AdminBillingCollectorRecord>>>>,
    #[cfg(test)]
    pub(crate) admin_security_blacklist_store: Option<Arc<StdMutex<HashMap<String, String>>>>,
    #[cfg(test)]
    pub(crate) admin_security_whitelist_store:
        Option<Arc<StdMutex<std::collections::BTreeSet<String>>>>,
    #[cfg(test)]
    pub(crate) admin_monitoring_cache_affinity_store:
        Option<Arc<StdMutex<HashMap<String, String>>>>,
    #[cfg(test)]
    pub(crate) admin_monitoring_redis_key_store: Option<Arc<StdMutex<HashMap<String, String>>>>,
    #[cfg(test)]
    pub(crate) provider_oauth_token_url_overrides: Arc<StdMutex<HashMap<String, String>>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_PROFILE: GateAutoProfile = GateAutoProfile {
        floor: 10_000,
        cap: 16_384,
        per_cpu: 1024,
        fd_divisor: Some(2),
    };

    const TEST_CAPACITY: GateAutoCapacity = GateAutoCapacity {
        cpu_parallelism: 12,
        fd_soft_limit: 1_048_576,
    };

    #[test]
    fn gate_limit_parser_defaults_to_auto() {
        assert_eq!(
            parse_gate_limit_value(None, TEST_PROFILE, TEST_CAPACITY),
            Some(12_288)
        );
        assert_eq!(
            parse_gate_limit_value(Some("auto"), TEST_PROFILE, TEST_CAPACITY),
            Some(12_288)
        );
        assert_eq!(
            parse_gate_limit_value(Some("  AUTO  "), TEST_PROFILE, TEST_CAPACITY),
            Some(12_288)
        );
    }

    #[test]
    fn auth_snapshot_load_gate_auto_limit_uses_smaller_frontdoor_profile() {
        assert_eq!(
            parse_gate_limit_value(
                Some("auto"),
                AUTH_SNAPSHOT_LOAD_GATE_AUTO_PROFILE,
                TEST_CAPACITY
            ),
            Some(192)
        );
    }

    #[test]
    fn gate_limit_parser_accepts_fixed_numbers() {
        assert_eq!(
            parse_gate_limit_value(Some("4096"), TEST_PROFILE, TEST_CAPACITY),
            Some(4096)
        );
    }

    #[test]
    fn gate_limit_parser_accepts_off_and_legacy_zero() {
        for value in ["off", "none", "disabled", "disable", "0"] {
            assert_eq!(
                parse_gate_limit_value(Some(value), TEST_PROFILE, TEST_CAPACITY),
                None
            );
        }
    }

    #[test]
    fn auto_gate_limit_respects_fd_budget_when_fd_limit_is_low() {
        assert_eq!(
            parse_gate_limit_value(
                Some("auto"),
                TEST_PROFILE,
                GateAutoCapacity {
                    cpu_parallelism: 32,
                    fd_soft_limit: 1024,
                },
            ),
            Some(448)
        );
    }
}
