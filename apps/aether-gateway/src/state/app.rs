use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use aether_runtime::{ConcurrencyGate, DistributedConcurrencyGate};

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
pub struct AppState {
    #[cfg(test)]
    pub(crate) execution_runtime_override_base_url: Option<String>,
    #[cfg(test)]
    pub(crate) execution_runtime_sync_override: Option<TestExecutionRuntimeSyncOverride>,
    pub(crate) data: Arc<GatewayDataState>,
    pub(crate) usage_runtime: Arc<usage::UsageRuntime>,
    pub(crate) video_tasks: Arc<VideoTaskService>,
    pub(crate) video_task_poller: Option<VideoTaskPollerConfig>,
    pub(crate) request_gate: Option<Arc<ConcurrencyGate>>,
    pub(crate) distributed_request_gate: Option<Arc<DistributedConcurrencyGate>>,
    pub(crate) client: reqwest::Client,
    pub(crate) auth_context_cache: Arc<AuthContextCache>,
    pub(crate) auth_api_key_last_used_cache: Arc<AuthApiKeyLastUsedCache>,
    pub(crate) oauth_refresh: Arc<provider_transport::LocalOAuthRefreshCoordinator>,
    pub(crate) direct_plan_bypass_cache: Arc<DirectPlanBypassCache>,
    pub(crate) scheduler_affinity_cache: Arc<SchedulerAffinityCache>,
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
