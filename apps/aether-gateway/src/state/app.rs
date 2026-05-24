use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use aether_runtime::ConcurrencyGate;
use aether_runtime_state::{RuntimeSemaphore, RuntimeState};

use super::super::async_task::{VideoTaskPollerConfig, VideoTaskService};
use super::super::cache::{
    AuthApiKeyLastUsedCache, AuthContextCache, DashboardResponseCache, DirectPlanBypassCache,
    SchedulerAffinityCache, SystemConfigCache,
};
use super::super::data::GatewayDataState;
use super::super::fallback_metrics;
use super::super::rate_limit::FrontdoorUserRpmLimiter;
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

const DEFAULT_LOCAL_EXECUTION_PLANNING_TIMEOUT_MS: u64 = 30_000;
const MIN_LOCAL_EXECUTION_PLANNING_TIMEOUT_MS: u64 = 500;
const MAX_LOCAL_EXECUTION_PLANNING_TIMEOUT_MS: u64 = 120_000;
const LOCAL_EXECUTION_PLANNING_TIMEOUT_MS_ENV: &str =
    "AETHER_GATEWAY_LOCAL_EXECUTION_PLANNING_TIMEOUT_MS";

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
    pub(crate) local_execution_planning_timeout: Duration,
}

impl FrontdoorRuntimeGuardConfig {
    pub(crate) fn from_env() -> Self {
        Self {
            request_body_read_timeout: env_duration_ms(
                REQUEST_BODY_READ_TIMEOUT_MS_ENV,
                DEFAULT_REQUEST_BODY_READ_TIMEOUT_MS,
                MIN_REQUEST_BODY_READ_TIMEOUT_MS,
                MAX_REQUEST_BODY_READ_TIMEOUT_MS,
            ),
            local_execution_planning_timeout: env_duration_ms(
                LOCAL_EXECUTION_PLANNING_TIMEOUT_MS_ENV,
                DEFAULT_LOCAL_EXECUTION_PLANNING_TIMEOUT_MS,
                MIN_LOCAL_EXECUTION_PLANNING_TIMEOUT_MS,
                MAX_LOCAL_EXECUTION_PLANNING_TIMEOUT_MS,
            ),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_tests(
        request_body_read_timeout: Duration,
        local_execution_planning_timeout: Duration,
    ) -> Self {
        Self {
            request_body_read_timeout,
            local_execution_planning_timeout,
        }
    }
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

#[derive(Debug, Clone)]
pub struct AppState {
    #[cfg(test)]
    pub(crate) execution_runtime_override_base_url: Option<String>,
    #[cfg(test)]
    pub(crate) execution_runtime_sync_override: Option<TestExecutionRuntimeSyncOverride>,
    pub(crate) data: Arc<GatewayDataState>,
    pub(crate) runtime_state: Arc<RuntimeState>,
    pub(crate) usage_runtime: Arc<usage::UsageRuntime>,
    pub(crate) video_tasks: Arc<VideoTaskService>,
    pub(crate) video_task_poller: Option<VideoTaskPollerConfig>,
    pub(crate) frontdoor_runtime_guards: Arc<FrontdoorRuntimeGuardConfig>,
    pub(crate) request_gate: Option<Arc<ConcurrencyGate>>,
    pub(crate) distributed_request_gate: Option<Arc<RuntimeSemaphore>>,
    pub(crate) client: reqwest::Client,
    pub(crate) auth_context_cache: Arc<AuthContextCache>,
    pub(crate) auth_api_key_last_used_cache: Arc<AuthApiKeyLastUsedCache>,
    pub(crate) oauth_refresh: Arc<provider_transport::LocalOAuthRefreshCoordinator>,
    pub(crate) direct_plan_bypass_cache: Arc<DirectPlanBypassCache>,
    pub(crate) scheduler_affinity_cache: Arc<SchedulerAffinityCache>,
    pub(crate) scheduler_affinity_epoch: Arc<AtomicU64>,
    pub(crate) dashboard_response_cache: Arc<DashboardResponseCache>,
    pub(crate) system_config_cache: Arc<SystemConfigCache>,
    pub(crate) fallback_metrics: Arc<fallback_metrics::GatewayFallbackMetrics>,
    pub(crate) frontdoor_cors: Option<Arc<FrontdoorCorsConfig>>,
    pub(crate) frontdoor_user_rpm: Arc<FrontdoorUserRpmLimiter>,
    pub(crate) tunnel: crate::tunnel::EmbeddedTunnelState,
    pub(crate) provider_transport_snapshot_cache:
        Arc<StdMutex<HashMap<ProviderTransportSnapshotCacheKey, CachedProviderTransportSnapshot>>>,
    pub(crate) provider_key_rpm_resets: Arc<StdMutex<HashMap<String, u64>>>,
    pub(crate) local_execution_runtime_miss_diagnostics:
        Arc<StdMutex<HashMap<String, LocalExecutionRuntimeMissDiagnostic>>>,
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
