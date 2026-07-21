use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use aether_data::repository::proxy_nodes::{
    ProxyNodeEventQuery, ProxyNodeHeartbeatMutation, ProxyNodeManualCreateMutation,
    ProxyNodeManualUpdateMutation, ProxyNodeMetricsStep, ProxyNodeTrafficMutation,
    ProxyNodeTunnelStatusMutation, StoredProxyFleetMetricsBucket, StoredProxyNode,
    StoredProxyNodeEvent, StoredProxyNodeMetricsBucket,
};
use aether_data_contracts::repository::usage::{
    UsageCounterHealthSnapshot, UsageCounterPendingHealthSnapshot,
};
use aether_http::{build_http_client, HttpClientConfig};
use aether_runtime::{
    service_up_sample, AdmissionPermit, ConcurrencyGate, ConcurrencySnapshot, MetricKind,
    MetricLabel, MetricSample,
};
use aether_runtime_state::{
    MemoryRuntimeStateConfig, RedisRuntimeDiagnostics, RuntimeQueueStore, RuntimeSemaphore,
    RuntimeSemaphoreError, RuntimeSemaphoreSnapshot, RuntimeState,
};
use aether_scheduler_core::PROVIDER_KEY_RPM_WINDOW_SECS;
use dashmap::DashMap;
use tokio::sync::{Mutex as TokioMutex, RwLock as TokioRwLock};
use tracing::warn;

use super::app::METRIC_SNAPSHOT_TTL;
use super::{
    AppState, FrontdoorCorsConfig, FrontdoorRuntimeGuardConfig, LocalExecutionRuntimeMissDiagnostic,
};

use super::super::async_task::{
    spawn_video_task_poller, VideoTaskPollerConfig, VideoTaskService, VideoTaskTruthSourceMode,
};
use super::super::cache::{
    AuthApiKeyLastUsedCache, AuthContextCache, AuthSnapshotCache, DashboardResponseCache,
    DirectPlanBypassCache, JsonValueCache, SchedulerAffinityCache, SchedulerAffinitySnapshotEntry,
    SchedulerAffinityTarget, SystemConfigCache, SystemConfigInflightRegistration, ValueCache,
};
use super::super::data::{GatewayDataConfig, GatewayDataState};
use super::super::fallback_metrics;
use super::super::fallback_metrics::{GatewayFallbackMetricKind, GatewayFallbackReason};
use super::super::model_fetch::spawn_model_fetch_worker;
use super::super::rate_limit::{FrontdoorUserRpmConfig, FrontdoorUserRpmLimiter};
use super::super::request_candidate_queue::{
    RequestCandidateQueueConfig, RequestCandidateQueueRuntime,
};
use super::super::router::RequestAdmissionError;
use super::super::{control::GatewayControlDecision, error::GatewayError};
use super::super::{provider_transport, usage};

use crate::maintenance::spawn_account_self_check_worker;
use crate::maintenance::spawn_audit_cleanup_worker;
use crate::maintenance::spawn_db_maintenance_worker;
use crate::maintenance::spawn_fixed_provider_reconciliation_task;
use crate::maintenance::spawn_gemini_file_mapping_cleanup_worker;
use crate::maintenance::spawn_oauth_token_refresh_worker;
use crate::maintenance::spawn_pending_cleanup_worker;
use crate::maintenance::spawn_pool_monitor_worker;
use crate::maintenance::spawn_pool_quota_probe_worker;
use crate::maintenance::spawn_pool_score_rebuild_worker;
use crate::maintenance::spawn_provider_checkin_worker;
use crate::maintenance::spawn_provider_quota_alert_worker;
use crate::maintenance::spawn_proxy_node_metrics_cleanup_worker;
use crate::maintenance::spawn_proxy_node_stale_cleanup_worker;
use crate::maintenance::spawn_proxy_upgrade_rollout_worker;
use crate::maintenance::spawn_request_candidate_cleanup_worker;
use crate::maintenance::spawn_stats_aggregation_worker;
use crate::maintenance::spawn_stats_hourly_aggregation_worker;
use crate::maintenance::spawn_usage_cleanup_worker;
use crate::maintenance::spawn_usage_counter_flush_worker;
use crate::maintenance::spawn_wallet_daily_usage_aggregation_worker;

const SYSTEM_CONFIG_CACHE_TTL: Duration = Duration::from_secs(30);
const SCHEDULER_AFFECTING_SYSTEM_CONFIG_KEYS: &[&str] = &[
    "enable_format_conversion",
    "keep_priority_on_conversion",
    "provider_priority_mode",
    "scheduling_mode",
];
const AUTH_AFFECTING_SYSTEM_CONFIG_KEYS: &[&str] = &[
    crate::constants::DEFAULT_USER_GROUP_CONFIG_KEY,
    crate::constants::ANTIGRAVITY_BEARER_BRIDGE_CONFIG_KEY,
];
const FRONTDOOR_RPM_AFFECTING_SYSTEM_CONFIG_KEYS: &[&str] = &["rate_limit_per_minute"];
const CHAT_PII_REDACTION_SYSTEM_CONFIG_PREFIX: &str = "module.chat_pii_redaction.";
const METRIC_SNAPSHOT_REFRESH_TIMEOUT: Duration = Duration::from_secs(4);
const METRIC_SNAPSHOT_PREWARM_TIMEOUT: Duration = Duration::from_secs(12);
// Dependency collection runs behind the stale-while-revalidate snapshot and on the isolated
// background pool, so this budget does not extend the HTTP scrape latency. Keep enough headroom
// for a short executor or host scheduling pause at the 10k-stream fan-in point while remaining
// below the outer snapshot refresh deadline.
const POSTGRES_OBSERVABILITY_METRICS_TIMEOUT: Duration = Duration::from_secs(2);
const POSTGRES_ACTIVITY_GROUP_METRICS_TIMEOUT: Duration = Duration::from_secs(2);
const POSTGRES_ACTIVITY_GROUP_METRICS_LIMIT: i64 = 8;
const REDIS_RUNTIME_METRICS_TIMEOUT: Duration = Duration::from_secs(2);
const DISTRIBUTED_CONCURRENCY_METRICS_TIMEOUT: Duration = Duration::from_millis(500);
const USAGE_QUEUE_HEALTH_METRICS_TIMEOUT: Duration = Duration::from_secs(2);
const USAGE_COUNTER_HEALTH_METRICS_TIMEOUT: Duration = Duration::from_secs(2);
const USAGE_COUNTER_EXACT_HEALTH_METRICS_TIMEOUT: Duration = Duration::from_secs(10);
const USAGE_COUNTER_EXACT_HEALTH_METRICS_TTL: Duration = Duration::from_secs(5 * 60);

fn system_config_key_affects_scheduler(key: &str) -> bool {
    let key = key.trim();
    SCHEDULER_AFFECTING_SYSTEM_CONFIG_KEYS.contains(&key)
}

fn system_config_key_affects_auth(key: &str) -> bool {
    let key = key.trim();
    AUTH_AFFECTING_SYSTEM_CONFIG_KEYS.contains(&key)
}

fn system_config_key_affects_frontdoor_rpm(key: &str) -> bool {
    let key = key.trim();
    FRONTDOOR_RPM_AFFECTING_SYSTEM_CONFIG_KEYS.contains(&key)
}

fn system_config_key_affects_chat_pii_redaction(key: &str) -> bool {
    key.trim()
        .starts_with(CHAT_PII_REDACTION_SYSTEM_CONFIG_PREFIX)
}

fn system_config_key_affects_provider_transport_snapshot(key: &str) -> bool {
    key.trim() == "enable_format_conversion"
}

impl AppState {
    pub async fn prewarm_chat_pii_redaction_runtime_config(&self) -> Result<bool, String> {
        crate::privacy::read_chat_pii_redaction_runtime_config(self)
            .await
            .map(|config| config.enabled)
            .map_err(|err| format!("{err:?}"))
    }

    fn usage_worker_queue_for(
        runtime_state: &Arc<RuntimeState>,
    ) -> Option<Arc<dyn RuntimeQueueStore>> {
        let queue: Arc<dyn RuntimeQueueStore> = runtime_state.clone();
        Some(queue)
    }

    fn spawn_scheduler_affinity_runtime_write(
        &self,
        cache_key: &str,
        target: &SchedulerAffinityTarget,
        ttl: Duration,
        epoch: u64,
    ) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };

        let cache_key = cache_key.to_string();
        let runtime_state = self.runtime_state.clone();
        let scheduler_affinity_epoch = self.scheduler_affinity_epoch.clone();
        let provider_id = target.provider_id.clone();
        let endpoint_id = target.endpoint_id.clone();
        let key_id = target.key_id.clone();
        let ttl_seconds = ttl.as_secs();
        let now_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;
        let expire_at = now_unix_secs.saturating_add(ttl_seconds);

        handle.spawn(async move {
            if scheduler_affinity_epoch.load(Ordering::Acquire) != epoch {
                return;
            }
            let existing = runtime_state
                .kv_get(&cache_key)
                .await
                .ok()
                .flatten()
                .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok());
            let request_count = existing
                .as_ref()
                .and_then(|value| value.get("request_count"))
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default()
                .saturating_add(1);
            let created_at = existing
                .as_ref()
                .and_then(|value| value.get("created_at"))
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(now_unix_secs);
            let payload = serde_json::json!({
                "provider_id": provider_id,
                "endpoint_id": endpoint_id,
                "key_id": key_id,
                "created_at": created_at,
                "expire_at": expire_at,
                "request_count": request_count,
                "scheduler_affinity_epoch": epoch,
            });
            if let Ok(serialized) = serde_json::to_string(&payload) {
                if scheduler_affinity_epoch.load(Ordering::Acquire) != epoch {
                    return;
                }
                let _ = runtime_state
                    .kv_set(
                        &cache_key,
                        serialized,
                        Some(Duration::from_secs(ttl_seconds)),
                    )
                    .await;
            }
        });
    }

    pub(crate) fn replace_data_state(&mut self, data: Arc<GatewayDataState>) {
        self.background_data = Arc::new(
            (*data)
                .clone()
                .with_usage_worker_queue(Self::usage_worker_queue_for(&self.runtime_state)),
        );
        self.background_data_isolated = false;
        self.replace_foreground_data_state(data);
    }

    fn replace_data_states(
        &mut self,
        data: Arc<GatewayDataState>,
        background_data: Arc<GatewayDataState>,
        background_data_isolated: bool,
    ) {
        self.background_data = Arc::new(
            (*background_data)
                .clone()
                .with_usage_worker_queue(Self::usage_worker_queue_for(&self.runtime_state)),
        );
        self.background_data_isolated = background_data_isolated;
        self.replace_foreground_data_state(data);
    }

    fn replace_foreground_data_state(&mut self, data: Arc<GatewayDataState>) {
        self.clear_provider_transport_snapshot_cache();
        self.invalidate_scheduler_affinity_cache();
        self.invalidate_auth_context_cache();
        self.candidate_row_page_cache.clear();
        self.candidate_resolved_page_cache.clear();
        self.system_config_cache.clear();
        self.frontdoor_user_rpm.clear_system_default_cache();
        let data = Arc::new(
            (*data)
                .clone()
                .with_usage_worker_queue(Self::usage_worker_queue_for(&self.runtime_state)),
        );
        self.candidate_row_page_cache.clear();
        self.candidate_page_cache.clear();
        self.candidate_resolved_page_cache.clear();
        self.tunnel = crate::tunnel::EmbeddedTunnelState::with_data_and_runtime_state(
            Arc::clone(&data),
            self.runtime_state.clone(),
        );
        self.data = data;
        self.configure_request_candidate_queue_from_env();
    }

    pub fn force_close_all_tunnel_proxies(&self) -> usize {
        self.tunnel.request_close_all_proxies()
    }

    pub fn new() -> Result<Self, reqwest::Error> {
        Self::build(None)
    }

    #[cfg(test)]
    pub(crate) fn with_execution_runtime_override_base_url(
        mut self,
        execution_runtime_override_base_url: impl Into<String>,
    ) -> Self {
        self.execution_runtime_override_base_url = Some(
            execution_runtime_override_base_url
                .into()
                .trim_end_matches('/')
                .to_string(),
        )
        .filter(|value| !value.is_empty());
        self
    }

    fn build(execution_runtime_override_base_url: Option<String>) -> Result<Self, reqwest::Error> {
        let runtime_state = Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let data = Arc::new(
            GatewayDataState::disabled()
                .with_usage_worker_queue(Self::usage_worker_queue_for(&runtime_state)),
        );
        let client = build_http_client(&HttpClientConfig {
            connect_timeout_ms: Some(10_000),
            request_timeout_ms: Some(300_000),
            http2_adaptive_window: true,
            ..HttpClientConfig::default()
        })?;
        let owner_forward_client = build_http_client(&HttpClientConfig {
            connect_timeout_ms: Some(10_000),
            http2_adaptive_window: true,
            ..HttpClientConfig::default()
        })?;
        let frontdoor_runtime_guards = Arc::new(FrontdoorRuntimeGuardConfig::from_env());
        Ok(Self {
            #[cfg(test)]
            execution_runtime_override_base_url: execution_runtime_override_base_url
                .map(|value| value.trim_end_matches('/').to_string())
                .filter(|value| !value.is_empty()),
            #[cfg(test)]
            execution_runtime_sync_override: None,
            data: Arc::clone(&data),
            background_data: Arc::clone(&data),
            background_data_isolated: false,
            runtime_state: runtime_state.clone(),
            usage_runtime: Arc::new(usage::UsageRuntime::disabled()),
            video_tasks: Arc::new(VideoTaskService::new(
                VideoTaskTruthSourceMode::PythonSyncReport,
            )),
            video_task_poller: None,
            frontdoor_runtime_guards: Arc::clone(&frontdoor_runtime_guards),
            request_body_buffer_budget: Arc::new(tokio::sync::Semaphore::new(
                frontdoor_runtime_guards.request_body_buffer_budget_permits,
            )),
            request_gate: None,
            auth_snapshot_load_gate: frontdoor_runtime_guards
                .auth_snapshot_load_gate_limit
                .map(|limit| Arc::new(ConcurrencyGate::new("gateway_auth_snapshot_load", limit))),
            candidate_planning_gate: frontdoor_runtime_guards
                .candidate_planning_gate_limit
                .map(|limit| Arc::new(ConcurrencyGate::new("gateway_candidate_planning", limit))),
            upstream_execution_gate: frontdoor_runtime_guards
                .upstream_execution_gate_limit
                .map(|limit| Arc::new(ConcurrencyGate::new("gateway_upstream_execution", limit))),
            upstream_target_admission: Arc::new(
                crate::upstream_admission::UpstreamTargetAdmission::new(
                    frontdoor_runtime_guards.upstream_target_gate_limit,
                    frontdoor_runtime_guards.internal_gate_queue_budget,
                ),
            ),
            distributed_request_gate: None,
            client,
            owner_forward_client,
            auth_context_cache: Arc::new(AuthContextCache::default()),
            auth_snapshot_cache: Arc::new(AuthSnapshotCache::default()),
            admin_security_blacklist_cache: Arc::new(ValueCache::default()),
            admin_security_whitelist_cache: Arc::new(ValueCache::default()),
            user_model_capability_settings_cache: Arc::new(JsonValueCache::default()),
            user_feature_settings_cache: Arc::new(JsonValueCache::default()),
            auth_api_key_force_capabilities_cache: Arc::new(JsonValueCache::default()),
            auth_api_key_feature_settings_cache: Arc::new(JsonValueCache::default()),
            auth_daily_quota_availability_cache: Arc::new(ValueCache::default()),
            auth_wallet_snapshot_cache: Arc::new(ValueCache::default()),
            auth_request_cost_upper_bound_cache: Arc::new(ValueCache::default()),
            provider_quota_snapshot_cache: Arc::new(ValueCache::default()),
            user_groups_for_user_cache: Arc::new(ValueCache::default()),
            routing_group_selection_cache: Arc::new(ValueCache::default()),
            auth_api_key_last_used_cache: Arc::new(AuthApiKeyLastUsedCache::default()),
            oauth_refresh: Arc::new(provider_transport::LocalOAuthRefreshCoordinator::new()),
            direct_plan_bypass_cache: Arc::new(DirectPlanBypassCache::default()),
            scheduler_affinity_cache: Arc::new(SchedulerAffinityCache::default()),
            scheduler_affinity_epoch: Arc::new(AtomicU64::new(0)),
            dashboard_response_cache: Arc::new(DashboardResponseCache::default()),
            system_config_cache: Arc::new(SystemConfigCache::default()),
            candidate_row_page_cache: Arc::new(crate::cache::CandidateRowPageCache::default()),
            candidate_page_cache: Arc::new(crate::cache::CandidatePageCache::default()),
            candidate_resolved_page_cache: Arc::new(
                crate::cache::CandidateResolvedPageCache::default(),
            ),
            chat_pii_redaction_runtime_config_cache:
                crate::privacy::new_chat_pii_redaction_runtime_config_cache(),
            fallback_metrics: Arc::new(fallback_metrics::GatewayFallbackMetrics::default()),
            usage_counter_flush_metrics: Arc::new(
                crate::maintenance::UsageCounterFlushRuntimeMetrics::default(),
            ),
            task_supervisor_metrics: crate::task_runtime::TaskSupervisorMetrics::default(),
            process_resource_monitor: Arc::new(
                crate::process_metrics::GatewayProcessResourceMonitor::new(),
            ),
            metric_snapshot: Arc::new(TokioRwLock::new(None)),
            metric_snapshot_refresh: Arc::new(TokioMutex::new(())),
            usage_counter_exact_health_metric_snapshot: Arc::new(TokioRwLock::new(None)),
            usage_counter_exact_health_metric_last_attempt: Arc::new(StdMutex::new(None)),
            usage_counter_exact_health_metric_refresh: Arc::new(TokioMutex::new(())),
            request_candidate_queue: None,
            frontdoor_cors: None,
            frontdoor_user_rpm: Arc::new(FrontdoorUserRpmLimiter::new(
                FrontdoorUserRpmConfig::default(),
            )),
            tunnel: crate::tunnel::EmbeddedTunnelState::with_data_and_runtime_state(
                data,
                runtime_state.clone(),
            ),
            provider_transport_snapshot_cache: Arc::new(DashMap::new()),
            provider_transport_snapshot_inflight: Arc::new(DashMap::new()),
            provider_key_rpm_resets: Arc::new(StdMutex::new(HashMap::new())),
            local_execution_runtime_miss_diagnostics: Arc::new(DashMap::new()),
            admin_monitoring_error_stats_reset_at: Arc::new(StdMutex::new(None)),
            provider_delete_tasks: Arc::new(StdMutex::new(HashMap::new())),
            #[cfg(test)]
            turnstile_siteverify_url_override: None,
            #[cfg(test)]
            turnstile_siteverify_timeout_override: None,
            #[cfg(test)]
            provider_oauth_state_store: None,
            #[cfg(test)]
            provider_oauth_device_session_store: Some(Arc::new(StdMutex::new(HashMap::new()))),
            #[cfg(test)]
            provider_oauth_batch_task_store: Some(Arc::new(StdMutex::new(HashMap::new()))),
            #[cfg(test)]
            auth_session_store: Some(Arc::new(StdMutex::new(HashMap::new()))),
            #[cfg(test)]
            auth_email_verification_store: Some(Arc::new(StdMutex::new(HashMap::new()))),
            #[cfg(test)]
            auth_email_delivery_store: Some(Arc::new(StdMutex::new(Vec::new()))),
            #[cfg(test)]
            auth_user_store: Some(Arc::new(StdMutex::new(HashMap::new()))),
            #[cfg(test)]
            auth_user_model_capability_store: Some(Arc::new(StdMutex::new(HashMap::new()))),
            #[cfg(test)]
            auth_wallet_store: Some(Arc::new(StdMutex::new(HashMap::new()))),
            #[cfg(test)]
            admin_wallet_payment_order_store: Some(Arc::new(StdMutex::new(HashMap::new()))),
            #[cfg(test)]
            admin_payment_callback_store: Some(Arc::new(StdMutex::new(HashMap::new()))),
            #[cfg(test)]
            admin_wallet_transaction_store: Some(Arc::new(StdMutex::new(HashMap::new()))),
            #[cfg(test)]
            admin_wallet_refund_store: Some(Arc::new(StdMutex::new(HashMap::new()))),
            #[cfg(test)]
            admin_billing_rule_store: Some(Arc::new(StdMutex::new(HashMap::new()))),
            #[cfg(test)]
            admin_billing_collector_store: Some(Arc::new(StdMutex::new(HashMap::new()))),
            #[cfg(test)]
            admin_security_blacklist_store: Some(Arc::new(StdMutex::new(HashMap::new()))),
            #[cfg(test)]
            admin_security_whitelist_store: Some(Arc::new(StdMutex::new(
                std::collections::BTreeSet::new(),
            ))),
            #[cfg(test)]
            admin_monitoring_cache_affinity_store: Some(Arc::new(StdMutex::new(HashMap::new()))),
            #[cfg(test)]
            admin_monitoring_redis_key_store: Some(Arc::new(StdMutex::new(HashMap::new()))),
            #[cfg(test)]
            provider_oauth_token_url_overrides: Arc::new(StdMutex::new(HashMap::new())),
        })
    }

    pub const fn execution_runtime_configured(&self) -> bool {
        true
    }

    #[cfg(test)]
    pub(crate) fn execution_runtime_override_base_url(&self) -> Option<&str> {
        self.execution_runtime_override_base_url.as_deref()
    }

    pub fn with_data_config(
        self,
        config: GatewayDataConfig,
    ) -> Result<Self, aether_data::DataLayerError> {
        self.with_data_config_and_background_isolation(config, true)
    }

    pub fn with_data_config_and_background_isolation(
        mut self,
        config: GatewayDataConfig,
        isolate_background: bool,
    ) -> Result<Self, aether_data::DataLayerError> {
        let (foreground_config, background_config) = if isolate_background {
            config.split_runtime_pools()
        } else {
            (config, None)
        };
        let auth_load_limit = database_bounded_auth_load_limit(
            self.frontdoor_runtime_guards.auth_snapshot_load_gate_limit,
            foreground_config
                .database()
                .map(|database| database.pool.max_connections),
        );
        self.auth_snapshot_load_gate = auth_load_limit
            .map(|limit| Arc::new(ConcurrencyGate::new("gateway_auth_snapshot_load", limit)));
        let background_data_isolated = background_config.is_some();
        let foreground_data = Arc::new(GatewayDataState::from_config(foreground_config)?);
        let background_data = match background_config {
            Some(config) => Arc::new(GatewayDataState::from_config(config)?),
            None => foreground_data.clone(),
        };
        self.replace_data_states(foreground_data, background_data, background_data_isolated);
        Ok(self)
    }

    pub fn with_tunnel_identity(
        mut self,
        instance_id: impl Into<String>,
        relay_base_url: Option<impl Into<String>>,
    ) -> Self {
        self.tunnel = crate::tunnel::EmbeddedTunnelState::with_data_identity_and_runtime_state(
            Arc::clone(&self.data),
            instance_id,
            relay_base_url,
            90,
            self.runtime_state.clone(),
        );
        self
    }

    pub fn with_video_task_truth_source_mode(mut self, mode: VideoTaskTruthSourceMode) -> Self {
        self.video_tasks = Arc::new(self.video_tasks.with_truth_source_mode(mode));
        self
    }

    pub fn with_usage_runtime_config(
        mut self,
        config: usage::UsageRuntimeConfig,
    ) -> Result<Self, aether_data::DataLayerError> {
        self.usage_runtime = Arc::new(usage::UsageRuntime::new(config)?);
        Ok(self)
    }

    pub async fn run_database_migrations(&self) -> Result<bool, sqlx::migrate::MigrateError> {
        self.data.run_database_migrations().await
    }

    pub async fn run_database_backfills(&self) -> Result<bool, sqlx::migrate::MigrateError> {
        self.data.run_database_backfills().await
    }

    pub async fn pending_database_migrations(
        &self,
    ) -> Result<
        Option<Vec<aether_data::lifecycle::migrate::PendingMigrationInfo>>,
        sqlx::migrate::MigrateError,
    > {
        self.data.pending_database_migrations().await
    }

    pub async fn prepare_database_for_startup(
        &self,
    ) -> Result<
        Option<Vec<aether_data::lifecycle::migrate::PendingMigrationInfo>>,
        sqlx::migrate::MigrateError,
    > {
        self.data.prepare_database_for_startup().await
    }

    pub async fn warm_database_pools(&self) -> Result<(), aether_data::DataLayerError> {
        self.data.warm_database_pool().await?;
        if self.background_data_isolated {
            self.background_data.warm_database_pool().await?;
        }
        Ok(())
    }

    pub async fn pending_database_backfills(
        &self,
    ) -> Result<
        Option<Vec<aether_data::lifecycle::backfill::PendingBackfillInfo>>,
        sqlx::migrate::MigrateError,
    > {
        self.data.pending_database_backfills().await
    }

    pub fn with_video_task_poller_config(mut self, interval: Duration, batch_size: usize) -> Self {
        self.video_task_poller = Some(VideoTaskPollerConfig {
            interval,
            batch_size: batch_size.max(1),
        });
        self
    }

    pub fn with_request_concurrency_limit(mut self, limit: usize) -> Self {
        self.request_gate = Some(Arc::new(ConcurrencyGate::new(
            "gateway_requests",
            limit.max(1),
        )));
        self
    }

    pub fn with_runtime_state(mut self, runtime_state: Arc<RuntimeState>) -> Self {
        self.runtime_state = runtime_state;
        self.admin_security_blacklist_cache.clear();
        self.admin_security_whitelist_cache.clear();
        self.data = Arc::new(
            (*self.data)
                .clone()
                .with_usage_worker_queue(Self::usage_worker_queue_for(&self.runtime_state)),
        );
        self.background_data = Arc::new(
            (*self.background_data)
                .clone()
                .with_usage_worker_queue(Self::usage_worker_queue_for(&self.runtime_state)),
        );
        self.tunnel = crate::tunnel::EmbeddedTunnelState::with_data_and_runtime_state(
            Arc::clone(&self.data),
            self.runtime_state.clone(),
        );
        self
    }

    fn configure_request_candidate_queue_from_env(&mut self) {
        let config = RequestCandidateQueueConfig::from_env();
        self.request_candidate_queue = if config.async_enabled() {
            if tokio::runtime::Handle::try_current().is_err() {
                warn!(
                    event_name = "request_candidate_async_queue_unavailable",
                    log_type = "ops",
                    "request candidate async queue requested outside a Tokio runtime; falling back to sync persistence"
                );
                None
            } else {
                self.request_candidate_queue_data_state()
                    .request_candidate_writer()
                    .map(|writer| RequestCandidateQueueRuntime::spawn(writer, config))
            }
        } else {
            None
        };
    }

    fn request_candidate_queue_data_state(&self) -> &Arc<GatewayDataState> {
        if self.background_data_isolated {
            &self.background_data
        } else {
            &self.data
        }
    }

    pub fn with_distributed_request_concurrency_gate(mut self, gate: RuntimeSemaphore) -> Self {
        self.distributed_request_gate = Some(Arc::new(gate));
        self
    }

    pub fn with_frontdoor_cors_config(mut self, config: FrontdoorCorsConfig) -> Self {
        self.frontdoor_cors = Some(Arc::new(config));
        self
    }

    pub fn with_frontdoor_user_rpm_config(mut self, config: FrontdoorUserRpmConfig) -> Self {
        self.frontdoor_user_rpm = Arc::new(FrontdoorUserRpmLimiter::new(config));
        self
    }

    pub fn has_data_backends(&self) -> bool {
        self.data.has_backends()
    }

    pub(crate) fn has_auth_api_key_reader(&self) -> bool {
        self.data.has_auth_api_key_reader()
    }

    pub(crate) fn has_proxy_node_reader(&self) -> bool {
        self.data.has_proxy_node_reader()
    }

    pub(crate) fn has_proxy_node_writer(&self) -> bool {
        self.data.has_proxy_node_writer()
    }

    pub(crate) fn frontdoor_cors(&self) -> Option<Arc<FrontdoorCorsConfig>> {
        self.frontdoor_cors.clone()
    }

    pub(crate) fn frontdoor_user_rpm(&self) -> Arc<FrontdoorUserRpmLimiter> {
        Arc::clone(&self.frontdoor_user_rpm)
    }

    pub(crate) fn mark_provider_key_rpm_reset(&self, key_id: &str, now_unix_secs: u64) {
        let mut resets = self
            .provider_key_rpm_resets
            .lock()
            .expect("provider key rpm reset cache should lock");
        let min_kept = now_unix_secs.saturating_sub(PROVIDER_KEY_RPM_WINDOW_SECS);
        resets.retain(|_, reset_at| *reset_at >= min_kept);
        resets.insert(key_id.to_string(), now_unix_secs);
    }

    pub(crate) fn provider_key_rpm_reset_at(
        &self,
        key_id: &str,
        now_unix_secs: u64,
    ) -> Option<u64> {
        let mut resets = self
            .provider_key_rpm_resets
            .lock()
            .expect("provider key rpm reset cache should lock");
        let min_kept = now_unix_secs.saturating_sub(PROVIDER_KEY_RPM_WINDOW_SECS);
        resets.retain(|_, reset_at| *reset_at >= min_kept);
        resets.get(key_id).copied()
    }

    pub(crate) fn admin_monitoring_error_stats_reset_at(&self) -> Option<u64> {
        *self
            .admin_monitoring_error_stats_reset_at
            .lock()
            .expect("admin monitoring error stats reset cache should lock")
    }

    pub(crate) fn mark_admin_monitoring_error_stats_reset(&self, now_unix_secs: u64) {
        let mut reset_at = self
            .admin_monitoring_error_stats_reset_at
            .lock()
            .expect("admin monitoring error stats reset cache should lock");
        *reset_at = Some(now_unix_secs);
    }

    pub(crate) async fn read_system_config_json_value(
        &self,
        key: &str,
    ) -> Result<Option<serde_json::Value>, GatewayError> {
        if let Some(value) = self.system_config_cache.get(key, SYSTEM_CONFIG_CACHE_TTL) {
            return Ok(value);
        }

        loop {
            let notified = self.system_config_cache.notified();
            match self.system_config_cache.register_load(key) {
                SystemConfigInflightRegistration::Bypass => {
                    let value = self
                        .data
                        .find_system_config_value(key)
                        .await
                        .map_err(|err| GatewayError::Internal(err.to_string()))?;
                    self.system_config_cache.insert(
                        key.to_string(),
                        value.clone(),
                        SYSTEM_CONFIG_CACHE_TTL,
                    );
                    return Ok(value);
                }
                SystemConfigInflightRegistration::Follower => {
                    notified.await;
                    if let Some(value) = self.system_config_cache.get(key, SYSTEM_CONFIG_CACHE_TTL)
                    {
                        return Ok(value);
                    }
                }
                SystemConfigInflightRegistration::Leader(_guard) => {
                    let value = self
                        .data
                        .find_system_config_value(key)
                        .await
                        .map_err(|err| GatewayError::Internal(err.to_string()))?;
                    self.system_config_cache.insert(
                        key.to_string(),
                        value.clone(),
                        SYSTEM_CONFIG_CACHE_TTL,
                    );
                    return Ok(value);
                }
            }
        }
    }

    pub(crate) async fn upsert_system_config_json_value(
        &self,
        key: &str,
        value: &serde_json::Value,
        description: Option<&str>,
    ) -> Result<serde_json::Value, GatewayError> {
        let value = self
            .data
            .upsert_system_config_value(key, value, description)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        self.remember_system_config_write(key, Some(value.clone()));
        Ok(value)
    }

    pub(crate) async fn list_system_config_entries(
        &self,
    ) -> Result<Vec<crate::data::state::StoredSystemConfigEntry>, GatewayError> {
        self.data
            .list_system_config_entries()
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn upsert_system_config_entry(
        &self,
        key: &str,
        value: &serde_json::Value,
        description: Option<&str>,
    ) -> Result<crate::data::state::StoredSystemConfigEntry, GatewayError> {
        let entry = self
            .data
            .upsert_system_config_entry(key, value, description)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        self.remember_system_config_write(entry.key.as_str(), Some(entry.value.clone()));
        Ok(entry)
    }

    pub(crate) async fn delete_system_config_value(&self, key: &str) -> Result<bool, GatewayError> {
        let deleted = self
            .data
            .delete_system_config_value(key)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        self.system_config_cache
            .insert(key.to_string(), None, SYSTEM_CONFIG_CACHE_TTL);
        if deleted && system_config_key_affects_scheduler(key) {
            self.invalidate_scheduler_affinity_cache();
        }
        if deleted && system_config_key_affects_auth(key) {
            self.invalidate_auth_context_cache();
        }
        if deleted && system_config_key_affects_frontdoor_rpm(key) {
            self.frontdoor_user_rpm.clear_system_default_cache();
        }
        if deleted && system_config_key_affects_chat_pii_redaction(key) {
            crate::privacy::clear_chat_pii_redaction_runtime_config_cache(
                &self.chat_pii_redaction_runtime_config_cache,
            );
        }
        if deleted && system_config_key_affects_provider_transport_snapshot(key) {
            self.clear_provider_transport_snapshot_cache();
        }
        Ok(deleted)
    }

    pub(crate) fn invalidate_provider_routing_caches(&self) {
        self.data.clear_minimal_candidate_selection_cache();
        self.data.clear_routing_group_cache();
        self.data.clear_provider_catalog_cache();
        self.auth_request_cost_upper_bound_cache.clear();
        self.routing_group_selection_cache.clear();
        self.candidate_row_page_cache.clear();
        self.candidate_page_cache.clear();
        self.candidate_resolved_page_cache.clear();
        self.clear_provider_transport_snapshot_cache();
        self.invalidate_scheduler_affinity_cache();
    }

    pub(crate) fn invalidate_provider_health_routing_caches(&self) {
        self.data.clear_minimal_candidate_selection_cache();
        self.data.clear_provider_catalog_cache();
        self.candidate_row_page_cache.clear();
        self.candidate_page_cache.clear();
        self.candidate_resolved_page_cache.clear();
        self.clear_provider_transport_snapshot_cache();
    }

    pub(crate) fn invalidate_provider_runtime_state_caches(&self) {
        self.data.clear_minimal_candidate_selection_cache();
        self.data.clear_provider_catalog_cache();
        self.clear_provider_transport_snapshot_cache();
    }

    pub(crate) fn invalidate_auth_context_cache(&self) {
        self.auth_context_cache.clear();
        self.auth_snapshot_cache.clear();
        self.user_model_capability_settings_cache.clear();
        self.user_feature_settings_cache.clear();
        self.auth_api_key_force_capabilities_cache.clear();
        self.auth_api_key_feature_settings_cache.clear();
        self.auth_daily_quota_availability_cache.clear();
        self.auth_wallet_snapshot_cache.clear();
        self.auth_request_cost_upper_bound_cache.clear();
        self.provider_quota_snapshot_cache.clear();
        self.user_groups_for_user_cache.clear();
        self.routing_group_selection_cache.clear();
        self.candidate_row_page_cache.clear();
        self.candidate_page_cache.clear();
        self.candidate_resolved_page_cache.clear();
    }

    fn remember_system_config_write(&self, key: &str, value: Option<serde_json::Value>) {
        self.system_config_cache
            .insert(key.to_string(), value, SYSTEM_CONFIG_CACHE_TTL);
        if system_config_key_affects_scheduler(key) {
            self.invalidate_scheduler_affinity_cache();
        }
        if system_config_key_affects_auth(key) {
            self.invalidate_auth_context_cache();
        }
        if system_config_key_affects_frontdoor_rpm(key) {
            self.frontdoor_user_rpm.clear_system_default_cache();
        }
        if system_config_key_affects_chat_pii_redaction(key) {
            crate::privacy::clear_chat_pii_redaction_runtime_config_cache(
                &self.chat_pii_redaction_runtime_config_cache,
            );
        }
        if system_config_key_affects_provider_transport_snapshot(key) {
            self.clear_provider_transport_snapshot_cache();
        }
    }

    pub(crate) async fn read_admin_system_stats(
        &self,
    ) -> Result<aether_data::repository::system::AdminSystemStats, GatewayError> {
        self.data
            .read_admin_system_stats()
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn purge_admin_system_data(
        &self,
        target: aether_data::repository::system::AdminSystemPurgeTarget,
    ) -> Result<aether_data::repository::system::AdminSystemPurgeSummary, GatewayError> {
        let summary = self
            .data
            .purge_admin_system_data(target)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if matches!(
            target,
            aether_data::repository::system::AdminSystemPurgeTarget::Config
                | aether_data::repository::system::AdminSystemPurgeTarget::Users
                | aether_data::repository::system::AdminSystemPurgeTarget::Usage
                | aether_data::repository::system::AdminSystemPurgeTarget::Stats
        ) {
            self.system_config_cache.clear();
            self.invalidate_provider_routing_caches();
        }
        Ok(summary)
    }

    pub(crate) async fn export_admin_system_usage_aggregates(
        &self,
    ) -> Result<aether_data::repository::system::AdminSystemUsageAggregateSnapshot, GatewayError>
    {
        self.data
            .export_admin_system_usage_aggregates()
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn import_admin_system_usage_aggregates(
        &self,
        snapshot: &aether_data::repository::system::AdminSystemUsageAggregateSnapshot,
        user_id_map: &std::collections::BTreeMap<String, String>,
        api_key_id_map: &std::collections::BTreeMap<String, String>,
        mode: aether_data::repository::system::AdminSystemUsageAggregateImportMode,
    ) -> Result<aether_data::repository::system::AdminSystemUsageAggregateImportSummary, GatewayError>
    {
        self.data
            .import_admin_system_usage_aggregates(snapshot, user_id_map, api_key_id_map, mode)
            .await
            .map_err(|err| match err {
                aether_data::DataLayerError::InvalidInput(detail) => GatewayError::Client {
                    status: http::StatusCode::BAD_REQUEST,
                    message: detail,
                },
                other => GatewayError::Internal(other.to_string()),
            })
    }

    pub(crate) async fn run_admin_system_cleanup_once(
        &self,
    ) -> Result<crate::maintenance::AdminSystemCleanupSummary, GatewayError> {
        crate::maintenance::run_admin_system_cleanup_once(&self.data)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn rebuild_admin_stats_once(
        &self,
    ) -> Result<crate::maintenance::AdminStatsRebuildSummary, GatewayError> {
        crate::maintenance::rebuild_admin_stats_once(&self.data)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn find_proxy_node(
        &self,
        node_id: &str,
    ) -> Result<Option<StoredProxyNode>, GatewayError> {
        self.data
            .find_proxy_node(node_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_proxy_nodes(&self) -> Result<Vec<StoredProxyNode>, GatewayError> {
        self.data
            .list_proxy_nodes()
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_proxy_node_events(
        &self,
        node_id: &str,
        limit: usize,
    ) -> Result<Vec<StoredProxyNodeEvent>, GatewayError> {
        self.data
            .list_proxy_node_events(node_id, limit)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_proxy_node_events_filtered(
        &self,
        node_id: &str,
        query: &ProxyNodeEventQuery,
    ) -> Result<Vec<StoredProxyNodeEvent>, GatewayError> {
        self.data
            .list_proxy_node_events_filtered(node_id, query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_proxy_node_metrics(
        &self,
        node_id: &str,
        step: ProxyNodeMetricsStep,
        from_unix_secs: u64,
        to_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredProxyNodeMetricsBucket>, GatewayError> {
        self.data
            .list_proxy_node_metrics(node_id, step, from_unix_secs, to_unix_secs, limit)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_proxy_fleet_metrics(
        &self,
        step: ProxyNodeMetricsStep,
        from_unix_secs: u64,
        to_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredProxyFleetMetricsBucket>, GatewayError> {
        self.data
            .list_proxy_fleet_metrics(step, from_unix_secs, to_unix_secs, limit)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn register_proxy_node(
        &self,
        mutation: &aether_data::repository::proxy_nodes::ProxyNodeRegistrationMutation,
    ) -> Result<Option<StoredProxyNode>, GatewayError> {
        self.data
            .register_proxy_node(mutation)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn create_manual_proxy_node(
        &self,
        mutation: &ProxyNodeManualCreateMutation,
    ) -> Result<Option<StoredProxyNode>, GatewayError> {
        self.data
            .create_manual_proxy_node(mutation)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn update_manual_proxy_node(
        &self,
        mutation: &ProxyNodeManualUpdateMutation,
    ) -> Result<Option<StoredProxyNode>, GatewayError> {
        self.data
            .update_manual_proxy_node(mutation)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub async fn reset_stale_proxy_node_tunnel_statuses(&self) -> std::io::Result<usize> {
        self.data
            .reset_stale_proxy_node_tunnel_statuses()
            .await
            .map_err(|err| std::io::Error::other(err.to_string()))
    }

    pub(crate) async fn cleanup_proxy_node_metrics(
        &self,
        retain_1m_from_unix_secs: u64,
        retain_1h_from_unix_secs: u64,
        delete_limit: usize,
    ) -> Result<aether_data::repository::proxy_nodes::ProxyNodeMetricsCleanupSummary, GatewayError>
    {
        self.data
            .cleanup_proxy_node_metrics(
                retain_1m_from_unix_secs,
                retain_1h_from_unix_secs,
                delete_limit,
            )
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn apply_proxy_node_heartbeat(
        &self,
        mutation: &ProxyNodeHeartbeatMutation,
    ) -> Result<Option<StoredProxyNode>, GatewayError> {
        self.data
            .apply_proxy_node_heartbeat(mutation)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn record_proxy_node_traffic(
        &self,
        mutation: &ProxyNodeTrafficMutation,
    ) -> Result<bool, GatewayError> {
        self.data
            .record_proxy_node_traffic(mutation)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn unregister_proxy_node(
        &self,
        node_id: &str,
    ) -> Result<Option<StoredProxyNode>, GatewayError> {
        self.data
            .unregister_proxy_node(node_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn delete_proxy_node(
        &self,
        node_id: &str,
    ) -> Result<Option<StoredProxyNode>, GatewayError> {
        self.data
            .delete_proxy_node(node_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn update_proxy_node_remote_config(
        &self,
        mutation: &aether_data::repository::proxy_nodes::ProxyNodeRemoteConfigMutation,
    ) -> Result<Option<StoredProxyNode>, GatewayError> {
        self.data
            .update_proxy_node_remote_config(mutation)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn update_proxy_node_tunnel_status(
        &self,
        mutation: &ProxyNodeTunnelStatusMutation,
    ) -> Result<Option<StoredProxyNode>, GatewayError> {
        self.data
            .update_proxy_node_tunnel_status(mutation)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) fn request_concurrency_snapshot(&self) -> Option<ConcurrencySnapshot> {
        self.request_gate.as_ref().map(|gate| gate.snapshot())
    }

    pub(crate) fn auth_snapshot_load_concurrency_snapshot(&self) -> Option<ConcurrencySnapshot> {
        self.auth_snapshot_load_gate
            .as_ref()
            .map(|gate| gate.snapshot())
    }

    pub(crate) fn candidate_planning_concurrency_snapshot(&self) -> Option<ConcurrencySnapshot> {
        self.candidate_planning_gate
            .as_ref()
            .map(|gate| gate.snapshot())
    }

    pub(crate) fn upstream_execution_concurrency_snapshot(&self) -> Option<ConcurrencySnapshot> {
        self.upstream_execution_gate
            .as_ref()
            .map(|gate| gate.snapshot())
    }

    pub(crate) async fn distributed_request_concurrency_snapshot(
        &self,
    ) -> Result<Option<RuntimeSemaphoreSnapshot>, RuntimeSemaphoreError> {
        match self.distributed_request_gate.as_ref() {
            Some(gate) => gate.snapshot().await.map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn metric_samples(&self) -> Vec<MetricSample> {
        let now = std::time::Instant::now();
        let snapshot = self.metric_snapshot.read().await.clone();
        let needs_refresh = snapshot.as_ref().is_none_or(|(created_at, _)| {
            now.saturating_duration_since(*created_at) >= METRIC_SNAPSHOT_TTL
        });
        if needs_refresh {
            self.spawn_metric_snapshot_refresh();
        }
        snapshot
            .map(|(_, samples)| samples)
            .unwrap_or_else(|| vec![service_up_sample("aether-gateway")])
    }

    pub async fn prewarm_metric_snapshot(&self) -> bool {
        match tokio::time::timeout(METRIC_SNAPSHOT_PREWARM_TIMEOUT, async {
            let _refresh_guard = self.metric_snapshot_refresh.lock().await;
            let _exact_health_guard = self.usage_counter_exact_health_metric_refresh.lock().await;
            self.mark_usage_counter_exact_health_metric_attempt();
            self.refresh_usage_counter_exact_health_metric_snapshot()
                .await;
            self.collect_and_store_metric_snapshot().await;
        })
        .await
        {
            Ok(()) => true,
            Err(_) => {
                warn!(
                    timeout_ms = METRIC_SNAPSHOT_PREWARM_TIMEOUT.as_millis() as u64,
                    "gateway metric snapshot prewarm timed out; startup will continue"
                );
                false
            }
        }
    }

    fn spawn_metric_snapshot_refresh(&self) {
        let Ok(refresh_guard) = Arc::clone(&self.metric_snapshot_refresh).try_lock_owned() else {
            return;
        };
        let state = self.clone();
        tokio::spawn(async move {
            let _refresh_guard = refresh_guard;
            let now = std::time::Instant::now();
            if state
                .metric_snapshot
                .read()
                .await
                .as_ref()
                .is_some_and(|(created_at, _)| {
                    now.saturating_duration_since(*created_at) < METRIC_SNAPSHOT_TTL
                })
            {
                return;
            }
            if tokio::time::timeout(
                METRIC_SNAPSHOT_REFRESH_TIMEOUT,
                state.collect_and_store_metric_snapshot(),
            )
            .await
            .is_err()
            {
                warn!(
                    timeout_ms = METRIC_SNAPSHOT_REFRESH_TIMEOUT.as_millis() as u64,
                    "gateway metric snapshot background refresh timed out; retaining stale snapshot"
                );
            }
        });
    }

    async fn collect_and_store_metric_snapshot(&self) {
        let samples = self.collect_metric_samples().await;
        *self.metric_snapshot.write().await = Some((std::time::Instant::now(), samples));
    }

    fn mark_usage_counter_exact_health_metric_attempt(&self) {
        *self
            .usage_counter_exact_health_metric_last_attempt
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(std::time::Instant::now());
    }

    fn usage_counter_exact_health_metric_refresh_is_due(&self) -> bool {
        let last_attempt = *self
            .usage_counter_exact_health_metric_last_attempt
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        last_attempt.is_none_or(|last_attempt| {
            std::time::Instant::now().saturating_duration_since(last_attempt)
                >= USAGE_COUNTER_EXACT_HEALTH_METRICS_TTL
        })
    }

    fn spawn_usage_counter_exact_health_metric_refresh(&self) {
        if !self.usage_counter_exact_health_metric_refresh_is_due() {
            return;
        }
        let Ok(refresh_guard) =
            Arc::clone(&self.usage_counter_exact_health_metric_refresh).try_lock_owned()
        else {
            return;
        };
        self.mark_usage_counter_exact_health_metric_attempt();
        let state = self.clone();
        tokio::spawn(async move {
            let _refresh_guard = refresh_guard;
            state
                .refresh_usage_counter_exact_health_metric_snapshot()
                .await;
        });
    }

    async fn refresh_usage_counter_exact_health_metric_snapshot(&self) -> bool {
        match tokio::time::timeout(
            USAGE_COUNTER_EXACT_HEALTH_METRICS_TIMEOUT,
            self.background_data.read_usage_counter_health(),
        )
        .await
        {
            Ok(Ok(snapshot)) => {
                *self
                    .usage_counter_exact_health_metric_snapshot
                    .write()
                    .await = Some((std::time::Instant::now(), snapshot));
                true
            }
            Ok(Err(err)) => {
                warn!(
                    error = %err,
                    "usage counter exact health metric refresh failed; retaining cached exact metrics"
                );
                false
            }
            Err(_) => {
                warn!(
                    timeout_ms = USAGE_COUNTER_EXACT_HEALTH_METRICS_TIMEOUT.as_millis() as u64,
                    "usage counter exact health metric refresh timed out; retaining cached exact metrics"
                );
                false
            }
        }
    }

    async fn usage_counter_exact_health_metric_samples(&self) -> Vec<MetricSample> {
        if self.usage_counter_exact_health_metric_refresh_is_due() {
            self.spawn_usage_counter_exact_health_metric_refresh();
        }
        self.usage_counter_exact_health_metric_snapshot
            .read()
            .await
            .as_ref()
            .map(|(created_at, snapshot)| {
                usage_counter_exact_health_metric_samples(
                    snapshot,
                    std::time::Instant::now()
                        .saturating_duration_since(*created_at)
                        .as_secs(),
                )
            })
            .unwrap_or_else(|| {
                vec![MetricSample::new(
                    "usage_counter_exact_health_unavailable",
                    "Whether the low-frequency exact usage counter health snapshot is unavailable.",
                    MetricKind::Gauge,
                    1,
                )]
            })
    }

    async fn collect_metric_samples(&self) -> Vec<MetricSample> {
        let mut samples = vec![service_up_sample("aether-gateway")];
        let request_body_buffer_budget_bytes = self
            .frontdoor_runtime_guards
            .request_body_buffer_budget_bytes;
        let request_body_buffer_available_bytes = self
            .request_body_buffer_budget
            .available_permits()
            .saturating_mul(super::REQUEST_BODY_BUFFER_PERMIT_BYTES)
            .min(request_body_buffer_budget_bytes);
        samples.extend([
            MetricSample::new(
                "request_body_buffer_budget_bytes",
                "Configured weighted request body buffering budget in bytes.",
                MetricKind::Gauge,
                u64::try_from(request_body_buffer_budget_bytes).unwrap_or(u64::MAX),
            ),
            MetricSample::new(
                "request_body_buffer_available_bytes",
                "Currently available weighted request body buffering budget in bytes.",
                MetricKind::Gauge,
                u64::try_from(request_body_buffer_available_bytes).unwrap_or(u64::MAX),
            ),
            MetricSample::new(
                "request_body_buffer_in_use_bytes",
                "Currently reserved weighted request body buffering budget in bytes.",
                MetricKind::Gauge,
                u64::try_from(
                    request_body_buffer_budget_bytes
                        .saturating_sub(request_body_buffer_available_bytes),
                )
                .unwrap_or(u64::MAX),
            ),
        ]);
        if let Some(snapshot) = self.request_concurrency_snapshot() {
            samples.extend(snapshot.to_metric_samples("gateway_requests"));
        }
        if let Some(snapshot) = self.auth_snapshot_load_concurrency_snapshot() {
            samples.extend(snapshot.to_metric_samples("gateway_auth_snapshot_load"));
        }
        if let Some(snapshot) = self.candidate_planning_concurrency_snapshot() {
            samples.extend(snapshot.to_metric_samples("gateway_candidate_planning"));
        }
        if let Some(snapshot) = self.upstream_execution_concurrency_snapshot() {
            samples.extend(snapshot.to_metric_samples("gateway_upstream_execution"));
        }
        if let Some(summary) = self.data.database_pool_summary() {
            samples.extend(database_pool_metric_samples(&summary));
        }
        samples.push(MetricSample::new(
            "background_database_pool_isolated",
            "Whether background workers use a database pool isolated from foreground traffic.",
            MetricKind::Gauge,
            u64::from(self.background_data_isolated),
        ));
        if self.background_data_isolated {
            if let Some(summary) = self.background_data.database_pool_summary() {
                samples.extend(background_database_pool_metric_samples(&summary));
            }
        }
        let distributed_request_metrics = async {
            let Some(gate) = self.distributed_request_gate.as_ref() else {
                return Vec::new();
            };
            match tokio::time::timeout(DISTRIBUTED_CONCURRENCY_METRICS_TIMEOUT, gate.snapshot())
                .await
            {
                Ok(Ok(snapshot)) => snapshot.to_metric_samples("gateway_requests_distributed"),
                Ok(Err(_)) | Err(_) => vec![MetricSample::new(
                    "concurrency_unavailable",
                    "Whether the distributed concurrency gate is currently unavailable.",
                    MetricKind::Gauge,
                    1,
                )
                .with_labels(vec![MetricLabel::new(
                    "gate",
                    "gateway_requests_distributed",
                )])],
            }
        };
        let postgres_observability_metrics = async {
            match tokio::time::timeout(
                POSTGRES_OBSERVABILITY_METRICS_TIMEOUT,
                self.background_data.postgres_observability_snapshot(),
            )
            .await
            {
                Ok(Ok(snapshot)) => postgres_observability_metric_samples(snapshot.as_ref()),
                Ok(Err(_)) | Err(_) => postgres_observability_unavailable_metric_samples(),
            }
        };
        let postgres_activity_group_metrics = async {
            match tokio::time::timeout(
                POSTGRES_ACTIVITY_GROUP_METRICS_TIMEOUT,
                self.background_data
                    .postgres_activity_groups(POSTGRES_ACTIVITY_GROUP_METRICS_LIMIT),
            )
            .await
            {
                Ok(Ok(groups)) => postgres_activity_group_metric_samples(&groups),
                Ok(Err(_)) | Err(_) => postgres_activity_group_unavailable_metric_samples(),
            }
        };
        let redis_runtime_metrics = async {
            match tokio::time::timeout(
                REDIS_RUNTIME_METRICS_TIMEOUT,
                self.runtime_state.redis_diagnostics(),
            )
            .await
            {
                Ok(Ok(snapshot)) => redis_runtime_metric_samples(snapshot.as_ref(), false),
                Ok(Err(_)) | Err(_) => {
                    redis_runtime_metric_samples(None, self.runtime_state.is_redis())
                }
            }
        };
        let usage_queue_health_metrics = usage_queue_health_metric_samples_with_timeout(
            USAGE_QUEUE_HEALTH_METRICS_TIMEOUT,
            self.usage_runtime
                .queue_health_snapshot(self.background_data.as_ref()),
        );
        let usage_counter_pending_health_metrics =
            usage_counter_pending_health_metric_samples_with_timeout(
                USAGE_COUNTER_HEALTH_METRICS_TIMEOUT,
                self.background_data.read_usage_counter_pending_health(),
                crate::clock::current_unix_secs(),
            );
        let (
            distributed_request_metrics,
            postgres_observability_metrics,
            postgres_activity_group_metrics,
            redis_runtime_metrics,
            usage_queue_health_metrics,
            usage_counter_pending_health_metrics,
        ) = tokio::join!(
            distributed_request_metrics,
            postgres_observability_metrics,
            postgres_activity_group_metrics,
            redis_runtime_metrics,
            usage_queue_health_metrics,
            usage_counter_pending_health_metrics,
        );
        samples.extend(distributed_request_metrics);
        samples.extend(postgres_observability_metrics);
        samples.extend(postgres_activity_group_metrics);
        samples.extend(redis_runtime_metrics);
        samples.extend(usage_queue_health_metrics);
        samples.extend(usage_counter_pending_health_metrics);
        if let Some(queue) = self.request_candidate_queue.as_ref() {
            samples.extend(queue.metric_samples());
        }
        samples.extend(usage_runtime_metric_samples(
            &self.usage_runtime.metrics_snapshot(),
        ));
        samples.extend(self.usage_counter_exact_health_metric_samples().await);
        samples.extend(self.usage_counter_flush_metrics.metric_samples());
        samples.extend(task_supervisor_metric_samples(
            &self.task_supervisor_metrics.snapshot(),
        ));
        samples.extend(crate::tokio_metrics::gateway_tokio_runtime_metric_samples());
        samples.extend(
            crate::execution_runtime::transport::direct_reqwest_client_cache_metric_samples(),
        );
        samples.extend(self.upstream_target_admission.metric_samples());
        samples.extend(crate::cache::candidate_page_cache_metric_samples());
        samples.extend(crate::stage_metrics::gateway_stage_metric_samples());
        samples.extend(self.tunnel.metric_samples());
        samples.extend(self.fallback_metrics.metric_samples());
        samples.extend(self.process_resource_monitor.metric_samples());
        samples.extend(crate::allocator_metrics::gateway_allocator_metric_samples());
        samples
    }

    pub(crate) fn record_fallback_metric(
        &self,
        kind: GatewayFallbackMetricKind,
        decision: Option<&GatewayControlDecision>,
        plan_kind: Option<&str>,
        execution_path: Option<&str>,
        reason: GatewayFallbackReason,
    ) {
        self.fallback_metrics
            .record(kind, decision, plan_kind, execution_path, reason);
    }

    pub(crate) fn clear_local_execution_runtime_miss_diagnostic(&self, trace_id: &str) {
        self.local_execution_runtime_miss_diagnostics
            .remove(trace_id);
    }

    pub(crate) fn set_local_execution_runtime_miss_diagnostic(
        &self,
        trace_id: &str,
        diagnostic: LocalExecutionRuntimeMissDiagnostic,
    ) {
        if self
            .local_execution_runtime_miss_diagnostics
            .get(trace_id)
            .is_some_and(|existing| {
                should_preserve_runtime_miss_diagnostic(existing.value(), &diagnostic)
            })
        {
            return;
        }
        self.local_execution_runtime_miss_diagnostics
            .insert(trace_id.to_string(), diagnostic);
    }

    pub(crate) fn mutate_local_execution_runtime_miss_diagnostic<F>(
        &self,
        trace_id: &str,
        mutate: F,
    ) where
        F: FnOnce(&mut LocalExecutionRuntimeMissDiagnostic),
    {
        if let Some(mut diagnostic) = self
            .local_execution_runtime_miss_diagnostics
            .get_mut(trace_id)
        {
            mutate(&mut diagnostic);
        }
    }

    pub(crate) fn local_execution_runtime_miss_diagnostic_has_candidate_signal(
        &self,
        trace_id: &str,
    ) -> bool {
        self.local_execution_runtime_miss_diagnostics
            .get(trace_id)
            .is_some_and(|diagnostic| {
                runtime_miss_diagnostic_has_candidate_signal(diagnostic.value())
            })
    }

    pub(crate) fn take_local_execution_runtime_miss_diagnostic(
        &self,
        trace_id: &str,
    ) -> Option<LocalExecutionRuntimeMissDiagnostic> {
        self.local_execution_runtime_miss_diagnostics
            .remove(trace_id)
            .map(|(_, diagnostic)| diagnostic)
    }

    pub(crate) async fn try_acquire_request_permit(
        &self,
    ) -> Result<Option<AdmissionPermit>, RequestAdmissionError> {
        let local = self
            .request_gate
            .as_ref()
            .map(|gate| gate.try_acquire())
            .transpose()
            .map_err(RequestAdmissionError::Local)?;
        let distributed = match self.distributed_request_gate.as_ref() {
            Some(gate) => Some(
                gate.try_acquire()
                    .await
                    .map_err(RequestAdmissionError::Distributed)?,
            ),
            None => None,
        };
        Ok(AdmissionPermit::from_parts(local, distributed))
    }

    pub fn has_auth_api_key_data_reader(&self) -> bool {
        self.data.has_auth_api_key_reader()
    }

    pub fn has_gemini_file_mapping_data_reader(&self) -> bool {
        self.data.has_gemini_file_mapping_reader()
    }

    pub fn has_gemini_file_mapping_data_writer(&self) -> bool {
        self.data.has_gemini_file_mapping_writer()
    }

    pub fn has_redis_data_backend(&self) -> bool {
        self.runtime_state.is_redis()
    }

    pub(crate) fn runtime_state_backend(&self) -> &'static str {
        self.runtime_state.backend_kind().as_str()
    }

    pub fn runtime_state(&self) -> &RuntimeState {
        self.runtime_state.as_ref()
    }

    pub(crate) async fn runtime_kv_setex(
        &self,
        key: &str,
        value: &str,
        ttl_seconds: u64,
    ) -> Result<(), GatewayError> {
        self.runtime_state
            .kv_set(
                key,
                value.to_string(),
                Some(Duration::from_secs(ttl_seconds)),
            )
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn runtime_kv_get(&self, key: &str) -> Result<Option<String>, GatewayError> {
        self.runtime_state
            .kv_get(key)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn runtime_kv_getdel(
        &self,
        key: &str,
    ) -> Result<Option<String>, GatewayError> {
        self.runtime_state
            .kv_take(key)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn runtime_kv_del(&self, key: &str) -> Result<bool, GatewayError> {
        self.runtime_state
            .kv_delete(key)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn runtime_kv_exists(&self, key: &str) -> Result<bool, GatewayError> {
        self.runtime_state
            .kv_exists(key)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) fn remove_scheduler_affinity_cache_entry(&self, cache_key: &str) -> bool {
        self.scheduler_affinity_cache.remove(cache_key).is_some()
    }

    pub(crate) fn scheduler_affinity_epoch(&self) -> u64 {
        self.scheduler_affinity_epoch.load(Ordering::Acquire)
    }

    pub(crate) fn invalidate_scheduler_affinity_cache(&self) -> u64 {
        let next_epoch = self
            .scheduler_affinity_epoch
            .fetch_add(1, Ordering::AcqRel)
            .saturating_add(1);
        self.scheduler_affinity_cache.clear();
        self.candidate_row_page_cache.clear();
        self.candidate_page_cache.clear();
        self.candidate_resolved_page_cache.clear();
        next_epoch
    }

    pub(crate) fn read_scheduler_affinity_target(
        &self,
        cache_key: &str,
        ttl: Duration,
    ) -> Option<SchedulerAffinityTarget> {
        self.scheduler_affinity_cache.get_fresh_for_epoch(
            cache_key,
            ttl,
            self.scheduler_affinity_epoch(),
        )
    }

    pub(crate) fn remember_scheduler_affinity_target(
        &self,
        cache_key: &str,
        target: SchedulerAffinityTarget,
        ttl: Duration,
        max_entries: usize,
    ) {
        let epoch = self.scheduler_affinity_epoch();
        self.remember_scheduler_affinity_target_for_epoch(
            cache_key,
            target,
            ttl,
            max_entries,
            Some(epoch),
        );
    }

    pub(crate) fn remember_scheduler_affinity_target_for_epoch(
        &self,
        cache_key: &str,
        target: SchedulerAffinityTarget,
        ttl: Duration,
        max_entries: usize,
        expected_epoch: Option<u64>,
    ) -> bool {
        let epoch = expected_epoch.unwrap_or_else(|| self.scheduler_affinity_epoch());
        if self.scheduler_affinity_epoch() != epoch {
            return false;
        }
        self.spawn_scheduler_affinity_runtime_write(cache_key, &target, ttl, epoch);
        self.scheduler_affinity_cache.insert_for_epoch(
            cache_key.to_string(),
            target,
            ttl,
            max_entries,
            epoch,
        );
        true
    }

    pub(crate) fn list_scheduler_affinity_entries(
        &self,
        ttl: Duration,
    ) -> Vec<SchedulerAffinitySnapshotEntry> {
        self.scheduler_affinity_cache
            .fresh_entries_for_epoch(ttl, self.scheduler_affinity_epoch())
    }

    pub fn with_video_task_store_path(
        mut self,
        path: impl Into<std::path::PathBuf>,
    ) -> std::io::Result<Self> {
        self.video_tasks = Arc::new(VideoTaskService::with_file_store(
            self.video_tasks.truth_source_mode(),
            path,
        )?);
        Ok(self)
    }

    fn background_worker_state(&self) -> Self {
        let mut state = self.clone();
        state.data = self.background_data.clone();
        state
    }

    pub fn spawn_background_tasks(&self) -> crate::task_runtime::TaskSupervisor {
        let background_state = self.background_worker_state();
        let mut supervisor =
            crate::task_runtime::TaskSupervisor::with_metrics(self.task_supervisor_metrics.clone());
        let record_boot = |task_key: &'static str| {
            if !background_state.has_background_task_data_writer() {
                return;
            }
            let Some(definition) = crate::task_runtime::task_definition(task_key) else {
                return;
            };
            std::mem::drop(crate::task_runtime::spawn_record_worker_boot(
                background_state.clone(),
                task_key,
                crate::task_runtime::background_task_kind(definition.kind),
                definition.trigger,
            ));
        };

        if let Some(handle) = self
            .usage_runtime
            .spawn_worker_supervisor(background_state.data.clone())
        {
            supervisor.supervise_handle(crate::task_runtime::TASK_KEY_USAGE_QUEUE_WORKER, handle);
            record_boot(crate::task_runtime::TASK_KEY_USAGE_QUEUE_WORKER);
        }

        if let Some(handle) = spawn_fixed_provider_reconciliation_task(background_state.clone()) {
            // This is a bounded startup reconciliation, not a long-running worker. Dropping a
            // Tokio JoinHandle detaches it; supervising it as a worker would incorrectly count
            // its successful completion as an unexpected background-task exit.
            std::mem::drop(handle);
        }

        let mut supervise_worker =
            |task_key: &'static str, handle: Option<tokio::task::JoinHandle<()>>| {
                if let Some(handle) = handle {
                    supervisor.supervise_handle(task_key, handle);
                    record_boot(task_key);
                }
            };

        supervise_worker(
            crate::task_runtime::TASK_KEY_USAGE_COUNTER_FLUSH,
            spawn_usage_counter_flush_worker(
                background_state.clone(),
                self.usage_counter_flush_metrics.clone(),
            ),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_PROVIDER_QUOTA_RESET,
            crate::wallet_runtime::spawn_provider_quota_reset_worker(background_state.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_AUDIT_CLEANUP,
            spawn_audit_cleanup_worker(background_state.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_DB_MAINTENANCE,
            spawn_db_maintenance_worker(background_state.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_WALLET_DAILY_USAGE_AGG,
            spawn_wallet_daily_usage_aggregation_worker(background_state.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_STATS_DAILY_AGG,
            spawn_stats_aggregation_worker(background_state.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_USAGE_CLEANUP,
            spawn_usage_cleanup_worker(background_state.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_POOL_MONITOR,
            spawn_pool_monitor_worker(background_state.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_ACCOUNT_SELF_CHECK,
            spawn_account_self_check_worker(background_state.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_POOL_SCORE_REBUILD,
            spawn_pool_score_rebuild_worker(background_state.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_POOL_QUOTA_PROBE,
            spawn_pool_quota_probe_worker(background_state.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_STATS_HOURLY_AGG,
            spawn_stats_hourly_aggregation_worker(background_state.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_PENDING_CLEANUP,
            spawn_pending_cleanup_worker(background_state.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_PROXY_NODE_STALE_CLEANUP,
            spawn_proxy_node_stale_cleanup_worker(background_state.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_PROXY_NODE_METRICS_CLEANUP,
            spawn_proxy_node_metrics_cleanup_worker(background_state.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_PROXY_UPGRADE_ROLLOUT,
            spawn_proxy_upgrade_rollout_worker(background_state.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_PROVIDER_CHECKIN,
            spawn_provider_checkin_worker(background_state.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_PROVIDER_QUOTA_ALERT,
            spawn_provider_quota_alert_worker(background_state.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_OAUTH_TOKEN_REFRESH,
            spawn_oauth_token_refresh_worker(background_state.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_REQUEST_CANDIDATE_CLEANUP,
            spawn_request_candidate_cleanup_worker(background_state.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_GEMINI_FILES_CLEANUP,
            spawn_gemini_file_mapping_cleanup_worker(background_state.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_MODEL_FETCH_WORKER,
            spawn_model_fetch_worker(background_state.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_VIDEO_TASK_POLLER,
            spawn_video_task_poller(background_state.clone()),
        );
        supervise_worker(
            crate::backup::worker::S3_BACKUP_WORKER_TASK_KEY,
            crate::backup::worker::spawn_s3_backup_worker(background_state.clone()),
        );

        supervisor
    }
}

fn database_bounded_auth_load_limit(
    configured_limit: Option<usize>,
    database_max_connections: Option<u32>,
) -> Option<usize> {
    configured_limit.map(|configured_limit| {
        let Some(database_max_connections) = database_max_connections else {
            return configured_limit.max(1);
        };
        let database_limit = (database_max_connections as usize / 2).max(1);
        configured_limit.max(1).min(database_limit)
    })
}

fn task_supervisor_metric_samples(
    snapshot: &aether_task_runtime::TaskSupervisorMetricsSnapshot,
) -> Vec<MetricSample> {
    let unexpected_exits_total = snapshot
        .completed_total
        .saturating_add(snapshot.panicked_total)
        .saturating_add(snapshot.aborted_total);
    let mut samples = vec![
        MetricSample::new(
            "gateway_background_tasks_active",
            "Current active background tasks supervised by the gateway task supervisor.",
            MetricKind::Gauge,
            snapshot.active_tasks,
        ),
        MetricSample::new(
            "gateway_background_tasks_supervised_total",
            "Total background tasks registered with the gateway task supervisor.",
            MetricKind::Counter,
            snapshot.supervised_total,
        ),
        MetricSample::new(
            "gateway_background_tasks_completed_total",
            "Total supervised gateway background tasks that returned without supervisor cancellation.",
            MetricKind::Counter,
            snapshot.completed_total,
        ),
        MetricSample::new(
            "gateway_background_tasks_panicked_total",
            "Total supervised gateway background tasks that ended with a panic.",
            MetricKind::Counter,
            snapshot.panicked_total,
        ),
        MetricSample::new(
            "gateway_background_tasks_aborted_total",
            "Total supervised gateway background tasks that were externally aborted.",
            MetricKind::Counter,
            snapshot.aborted_total,
        ),
        MetricSample::new(
            "gateway_background_tasks_cancelled_total",
            "Total supervised gateway background tasks cancelled by supervisor shutdown.",
            MetricKind::Counter,
            snapshot.cancelled_total,
        ),
        MetricSample::new(
            "gateway_background_tasks_unexpected_exits_total",
            "Total supervised gateway background tasks that exited without supervisor shutdown.",
            MetricKind::Counter,
            unexpected_exits_total,
        ),
    ];

    for task in &snapshot.tasks {
        let labels = vec![MetricLabel::new("task_key", task.task_name)];
        let task_unexpected_exits_total = task
            .completed_total
            .saturating_add(task.panicked_total)
            .saturating_add(task.aborted_total);
        samples.push(
            MetricSample::new(
                "gateway_background_task_active",
                "Current active supervised gateway background tasks by task key.",
                MetricKind::Gauge,
                task.active_tasks,
            )
            .with_labels(labels.clone()),
        );
        samples.push(
            MetricSample::new(
                "gateway_background_task_supervised_total",
                "Total supervised gateway background tasks registered by task key.",
                MetricKind::Counter,
                task.supervised_total,
            )
            .with_labels(labels.clone()),
        );
        samples.push(
            MetricSample::new(
                "gateway_background_task_completed_total",
                "Total supervised gateway background tasks that returned by task key.",
                MetricKind::Counter,
                task.completed_total,
            )
            .with_labels(labels.clone()),
        );
        samples.push(
            MetricSample::new(
                "gateway_background_task_panicked_total",
                "Total supervised gateway background tasks that panicked by task key.",
                MetricKind::Counter,
                task.panicked_total,
            )
            .with_labels(labels.clone()),
        );
        samples.push(
            MetricSample::new(
                "gateway_background_task_aborted_total",
                "Total supervised gateway background tasks externally aborted by task key.",
                MetricKind::Counter,
                task.aborted_total,
            )
            .with_labels(labels.clone()),
        );
        samples.push(
            MetricSample::new(
                "gateway_background_task_cancelled_total",
                "Total supervised gateway background tasks cancelled by supervisor shutdown by task key.",
                MetricKind::Counter,
                task.cancelled_total,
            )
            .with_labels(labels.clone()),
        );
        samples.push(
            MetricSample::new(
                "gateway_background_task_unexpected_exits_total",
                "Total supervised gateway background tasks that exited without supervisor shutdown by task key.",
                MetricKind::Counter,
                task_unexpected_exits_total,
            )
            .with_labels(labels.clone()),
        );
        samples.push(
            MetricSample::new(
                "gateway_background_task_singleton_lease_contention_total",
                "Total singleton lease acquisition attempts blocked by another owner by task key.",
                MetricKind::Counter,
                task.singleton_lease_contention_total,
            )
            .with_labels(labels.clone()),
        );
        samples.push(
            MetricSample::new(
                "gateway_background_task_singleton_lease_lost_total",
                "Total singleton leases lost while a gateway background task was running by task key.",
                MetricKind::Counter,
                task.singleton_lease_lost_total,
            )
            .with_labels(labels.clone()),
        );
        samples.push(
            MetricSample::new(
                "gateway_background_task_singleton_lease_error_total",
                "Total singleton lease backend errors observed by gateway background task key.",
                MetricKind::Counter,
                task.singleton_lease_error_total,
            )
            .with_labels(labels),
        );
    }

    samples
}

fn database_pool_metric_samples(summary: &aether_data::DatabasePoolSummary) -> Vec<MetricSample> {
    let labels = vec![MetricLabel::new("driver", summary.driver.to_string())];
    let usage_basis_points = if summary.usage_rate.is_finite() && summary.usage_rate > 0.0 {
        (summary.usage_rate * 100.0).round() as u64
    } else {
        0
    };
    let under_maintenance_pressure =
        GatewayDataState::database_pool_summary_under_maintenance_pressure(summary);

    vec![
        MetricSample::new(
            "usage_counter_exact_health_unavailable",
            "Whether the low-frequency exact usage counter health snapshot is unavailable.",
            MetricKind::Gauge,
            0,
        ),
        MetricSample::new(
            "database_pool_checked_out_connections",
            "Number of database connections currently checked out from the gateway pool.",
            MetricKind::Gauge,
            summary.checked_out as u64,
        )
        .with_labels(labels.clone()),
        MetricSample::new(
            "database_pool_idle_connections",
            "Number of idle database connections currently available in the gateway pool.",
            MetricKind::Gauge,
            summary.idle as u64,
        )
        .with_labels(labels.clone()),
        MetricSample::new(
            "database_pool_size_connections",
            "Current number of database connections opened by the gateway pool.",
            MetricKind::Gauge,
            summary.pool_size as u64,
        )
        .with_labels(labels.clone()),
        MetricSample::new(
            "database_pool_max_connections",
            "Configured maximum number of database connections for the gateway pool.",
            MetricKind::Gauge,
            summary.max_connections as u64,
        )
        .with_labels(labels.clone()),
        MetricSample::new(
            "database_pool_usage_basis_points",
            "Database pool usage rate in basis points, where 10000 means 100 percent.",
            MetricKind::Gauge,
            usage_basis_points,
        )
        .with_labels(labels.clone()),
        MetricSample::new(
            "database_pool_idle_reserve_connections",
            "Idle database connections reserved for foreground traffic before maintenance defers.",
            MetricKind::Gauge,
            GatewayDataState::maintenance_pool_idle_reserve(summary) as u64,
        )
        .with_labels(labels.clone()),
        MetricSample::new(
            "database_pool_under_maintenance_pressure",
            "Whether maintenance workers should currently defer for foreground database pool capacity.",
            MetricKind::Gauge,
            u64::from(under_maintenance_pressure),
        )
        .with_labels(labels),
    ]
}

fn background_database_pool_metric_samples(
    summary: &aether_data::DatabasePoolSummary,
) -> Vec<MetricSample> {
    let labels = vec![MetricLabel::new("driver", summary.driver.to_string())];
    let usage_basis_points = if summary.usage_rate.is_finite() && summary.usage_rate > 0.0 {
        (summary.usage_rate * 100.0).round() as u64
    } else {
        0
    };

    vec![
        MetricSample::new(
            "background_database_pool_checked_out_connections",
            "Number of database connections checked out from the isolated background pool.",
            MetricKind::Gauge,
            summary.checked_out as u64,
        )
        .with_labels(labels.clone()),
        MetricSample::new(
            "background_database_pool_idle_connections",
            "Number of idle database connections in the isolated background pool.",
            MetricKind::Gauge,
            summary.idle as u64,
        )
        .with_labels(labels.clone()),
        MetricSample::new(
            "background_database_pool_max_connections",
            "Configured maximum connections for the isolated background database pool.",
            MetricKind::Gauge,
            summary.max_connections as u64,
        )
        .with_labels(labels.clone()),
        MetricSample::new(
            "background_database_pool_usage_basis_points",
            "Background database pool usage rate in basis points, where 10000 means 100 percent.",
            MetricKind::Gauge,
            usage_basis_points,
        )
        .with_labels(labels),
    ]
}

fn postgres_observability_metric_samples(
    snapshot: Option<&aether_data::DatabasePostgresObservabilitySnapshot>,
) -> Vec<MetricSample> {
    let labels = vec![MetricLabel::new("driver", "postgres")];
    let available = u64::from(snapshot.is_some());
    let snapshot = snapshot.copied().unwrap_or_default();

    let mut samples = Vec::new();
    let mut push = |name: &'static str, description: &'static str, kind: MetricKind, value: u64| {
        samples.push(MetricSample::new(name, description, kind, value).with_labels(labels.clone()));
    };

    push(
        "postgres_observability_available",
        "Whether Postgres system catalog observability is configured and available.",
        MetricKind::Gauge,
        available,
    );
    push(
        "postgres_observability_unavailable",
        "Whether Postgres system catalog observability could not be read for this scrape.",
        MetricKind::Gauge,
        0,
    );
    push(
        "postgres_active_connections",
        "Number of active connections reported by pg_stat_activity for the current database.",
        MetricKind::Gauge,
        snapshot.active_connections,
    );
    push(
        "postgres_idle_connections",
        "Number of idle connections reported by pg_stat_activity for the current database.",
        MetricKind::Gauge,
        snapshot.idle_connections,
    );
    push(
        "postgres_idle_in_transaction_connections",
        "Number of idle-in-transaction connections reported by pg_stat_activity for the current database.",
        MetricKind::Gauge,
        snapshot.idle_in_transaction_connections,
    );
    push(
        "postgres_waiting_connections",
        "Number of connections currently waiting on an event in pg_stat_activity for the current database.",
        MetricKind::Gauge,
        snapshot.waiting_connections,
    );
    push(
        "postgres_lock_waiting_connections",
        "Number of connections currently waiting on locks in pg_stat_activity for the current database.",
        MetricKind::Gauge,
        snapshot.lock_waiting_connections,
    );
    push(
        "postgres_oldest_active_query_age_ms",
        "Age in milliseconds of the oldest active Postgres query for the current database.",
        MetricKind::Gauge,
        snapshot.oldest_active_query_age_ms,
    );
    push(
        "postgres_oldest_transaction_age_ms",
        "Age in milliseconds of the oldest Postgres transaction for the current database.",
        MetricKind::Gauge,
        snapshot.oldest_transaction_age_ms,
    );
    push(
        "postgres_deadlocks_total",
        "Total Postgres deadlocks reported by pg_stat_database for the current database.",
        MetricKind::Counter,
        snapshot.deadlocks_total,
    );
    push(
        "postgres_block_read_total",
        "Total Postgres heap/index blocks read from storage for the current database.",
        MetricKind::Counter,
        snapshot.block_read_total,
    );
    push(
        "postgres_block_hit_total",
        "Total Postgres heap/index block cache hits for the current database.",
        MetricKind::Counter,
        snapshot.block_hit_total,
    );
    push(
        "postgres_block_cache_hit_rate_basis_points",
        "Postgres block cache hit rate for the current database in basis points.",
        MetricKind::Gauge,
        snapshot.block_cache_hit_rate_basis_points,
    );
    push(
        "postgres_temp_files_total",
        "Total temporary files created by Postgres for the current database.",
        MetricKind::Counter,
        snapshot.temp_files_total,
    );
    push(
        "postgres_temp_bytes_total",
        "Total temporary bytes written by Postgres for the current database.",
        MetricKind::Counter,
        snapshot.temp_bytes_total,
    );
    push(
        "postgres_xact_commit_total",
        "Total committed Postgres transactions for the current database.",
        MetricKind::Counter,
        snapshot.xact_commit_total,
    );
    push(
        "postgres_xact_rollback_total",
        "Total rolled back Postgres transactions for the current database.",
        MetricKind::Counter,
        snapshot.xact_rollback_total,
    );
    push(
        "postgres_wal_observability_available",
        "Whether Postgres WAL statistics are available for this scrape.",
        MetricKind::Gauge,
        snapshot.wal_observability_available,
    );
    push(
        "postgres_wal_observability_unavailable",
        "Whether Postgres WAL statistics could not be read for this scrape.",
        MetricKind::Gauge,
        snapshot.wal_observability_unavailable,
    );
    push(
        "postgres_wal_records_total",
        "Total WAL records reported by Postgres.",
        MetricKind::Counter,
        snapshot.wal_records_total,
    );
    push(
        "postgres_wal_fpi_total",
        "Total WAL full-page images reported by Postgres.",
        MetricKind::Counter,
        snapshot.wal_fpi_total,
    );
    push(
        "postgres_wal_bytes_total",
        "Total WAL bytes reported by Postgres.",
        MetricKind::Counter,
        snapshot.wal_bytes_total,
    );
    push(
        "postgres_wal_buffers_full_total",
        "Total times WAL buffers were full in Postgres.",
        MetricKind::Counter,
        snapshot.wal_buffers_full_total,
    );
    push(
        "postgres_wal_write_total",
        "Total WAL write operations reported by Postgres.",
        MetricKind::Counter,
        snapshot.wal_write_total,
    );
    push(
        "postgres_wal_sync_total",
        "Total WAL sync operations reported by Postgres.",
        MetricKind::Counter,
        snapshot.wal_sync_total,
    );
    push(
        "postgres_wal_write_time_ms_total",
        "Total Postgres WAL write time in milliseconds.",
        MetricKind::Counter,
        snapshot.wal_write_time_ms_total,
    );
    push(
        "postgres_wal_sync_time_ms_total",
        "Total Postgres WAL sync time in milliseconds.",
        MetricKind::Counter,
        snapshot.wal_sync_time_ms_total,
    );
    push(
        "postgres_checkpoint_observability_available",
        "Whether Postgres checkpoint statistics are available for this scrape.",
        MetricKind::Gauge,
        snapshot.checkpoint_observability_available,
    );
    push(
        "postgres_checkpoint_observability_unavailable",
        "Whether Postgres checkpoint statistics could not be read for this scrape.",
        MetricKind::Gauge,
        snapshot.checkpoint_observability_unavailable,
    );
    push(
        "postgres_checkpoints_timed_total",
        "Total timed Postgres checkpoints reported by the checkpointer.",
        MetricKind::Counter,
        snapshot.checkpoints_timed_total,
    );
    push(
        "postgres_checkpoints_requested_total",
        "Total requested Postgres checkpoints reported by the checkpointer.",
        MetricKind::Counter,
        snapshot.checkpoints_requested_total,
    );
    push(
        "postgres_checkpoint_write_time_ms_total",
        "Total Postgres checkpoint write time in milliseconds.",
        MetricKind::Counter,
        snapshot.checkpoint_write_time_ms_total,
    );
    push(
        "postgres_checkpoint_sync_time_ms_total",
        "Total Postgres checkpoint sync time in milliseconds.",
        MetricKind::Counter,
        snapshot.checkpoint_sync_time_ms_total,
    );
    push(
        "postgres_buffers_checkpoint_total",
        "Total buffers written during Postgres checkpoints.",
        MetricKind::Counter,
        snapshot.buffers_checkpoint_total,
    );
    push(
        "postgres_buffers_backend_total",
        "Total Postgres buffers written by backend processes.",
        MetricKind::Counter,
        snapshot.buffers_backend_total,
    );
    push(
        "postgres_statement_observability_available",
        "Whether pg_stat_statements aggregate statistics are available for this scrape.",
        MetricKind::Gauge,
        snapshot.statement_observability_available,
    );
    push(
        "postgres_statement_observability_unavailable",
        "Whether pg_stat_statements aggregate statistics could not be read for this scrape.",
        MetricKind::Gauge,
        snapshot.statement_observability_unavailable,
    );
    push(
        "postgres_statement_top_calls_total",
        "Total calls across the top Postgres statements by execution time.",
        MetricKind::Counter,
        snapshot.statement_top_calls_total,
    );
    push(
        "postgres_statement_top_exec_time_ms_total",
        "Total execution time across the top Postgres statements by execution time.",
        MetricKind::Counter,
        snapshot.statement_top_exec_time_ms_total,
    );
    push(
        "postgres_statement_top_max_mean_exec_time_ms",
        "Maximum mean execution time among the top Postgres statements.",
        MetricKind::Gauge,
        snapshot.statement_top_max_mean_exec_time_ms,
    );
    push(
        "postgres_statement_top_max_exec_time_ms",
        "Maximum execution time among the top Postgres statements.",
        MetricKind::Gauge,
        snapshot.statement_top_max_exec_time_ms,
    );
    push(
        "postgres_statement_top_shared_blks_read_total",
        "Total shared blocks read across the top Postgres statements.",
        MetricKind::Counter,
        snapshot.statement_top_shared_blks_read_total,
    );
    push(
        "postgres_statement_top_shared_blks_hit_total",
        "Total shared block hits across the top Postgres statements.",
        MetricKind::Counter,
        snapshot.statement_top_shared_blks_hit_total,
    );
    push(
        "postgres_statement_top_temp_blks_total",
        "Total temporary blocks across the top Postgres statements.",
        MetricKind::Counter,
        snapshot.statement_top_temp_blks_total,
    );

    samples
}

fn postgres_observability_unavailable_metric_samples() -> Vec<MetricSample> {
    let labels = vec![MetricLabel::new("driver", "postgres")];
    vec![
        MetricSample::new(
            "postgres_observability_available",
            "Whether Postgres system catalog observability is configured and available.",
            MetricKind::Gauge,
            0,
        )
        .with_labels(labels.clone()),
        MetricSample::new(
            "postgres_observability_unavailable",
            "Whether Postgres system catalog observability could not be read for this scrape.",
            MetricKind::Gauge,
            1,
        )
        .with_labels(labels),
    ]
}

fn postgres_activity_group_metric_samples(
    groups: &[aether_data::DatabasePostgresActivityGroup],
) -> Vec<MetricSample> {
    let mut samples = vec![MetricSample::new(
        "postgres_activity_groups_available",
        "Whether grouped pg_stat_activity diagnostics are available for this scrape.",
        MetricKind::Gauge,
        1,
    )
    .with_labels(vec![MetricLabel::new("driver", "postgres")])];

    for (index, group) in groups.iter().enumerate() {
        let labels = vec![
            MetricLabel::new("driver", "postgres"),
            MetricLabel::new("rank", (index + 1).to_string()),
            MetricLabel::new("state", group.state.clone()),
            MetricLabel::new("wait_event_type", group.wait_event_type.clone()),
            MetricLabel::new("wait_event", group.wait_event.clone()),
            MetricLabel::new("query_prefix", group.query_prefix.clone()),
        ];
        samples.push(
            MetricSample::new(
                "postgres_activity_group_connections",
                "Connections in a grouped pg_stat_activity bucket ranked by connection count.",
                MetricKind::Gauge,
                group.connections,
            )
            .with_labels(labels.clone()),
        );
        samples.push(
            MetricSample::new(
                "postgres_activity_group_max_query_age_ms",
                "Maximum query age in milliseconds for a grouped pg_stat_activity bucket.",
                MetricKind::Gauge,
                group.max_query_age_ms,
            )
            .with_labels(labels.clone()),
        );
        samples.push(
            MetricSample::new(
                "postgres_activity_group_max_transaction_age_ms",
                "Maximum transaction age in milliseconds for a grouped pg_stat_activity bucket.",
                MetricKind::Gauge,
                group.max_transaction_age_ms,
            )
            .with_labels(labels),
        );
    }

    samples
}

fn postgres_activity_group_unavailable_metric_samples() -> Vec<MetricSample> {
    vec![MetricSample::new(
        "postgres_activity_groups_available",
        "Whether grouped pg_stat_activity diagnostics are available for this scrape.",
        MetricKind::Gauge,
        0,
    )
    .with_labels(vec![MetricLabel::new("driver", "postgres")])]
}

fn redis_runtime_metric_samples(
    snapshot: Option<&RedisRuntimeDiagnostics>,
    unavailable: bool,
) -> Vec<MetricSample> {
    let labels = vec![MetricLabel::new("backend", "redis")];
    let enabled = u64::from(snapshot.is_some() || unavailable);
    let mut samples = vec![
        MetricSample::new(
            "redis_runtime_enabled",
            "Whether the gateway runtime state backend is Redis.",
            MetricKind::Gauge,
            enabled,
        )
        .with_labels(labels.clone()),
        MetricSample::new(
            "redis_runtime_health_unavailable",
            "Whether Redis runtime diagnostics could not be read for this scrape.",
            MetricKind::Gauge,
            u64::from(unavailable),
        )
        .with_labels(labels.clone()),
    ];

    let Some(snapshot) = snapshot else {
        samples.extend(redis_runtime_zero_metric_samples(labels));
        return samples;
    };

    let keyspace_total = snapshot
        .keyspace_hits
        .unwrap_or_default()
        .saturating_add(snapshot.keyspace_misses.unwrap_or_default());
    let hit_rate_basis_points = snapshot
        .keyspace_hits
        .unwrap_or_default()
        .saturating_mul(10_000)
        .checked_div(keyspace_total)
        .unwrap_or_default();
    let memory_usage_basis_points = snapshot
        .maxmemory_bytes
        .filter(|maxmemory| *maxmemory > 0)
        .map(|maxmemory| {
            snapshot
                .used_memory_bytes
                .unwrap_or_default()
                .saturating_mul(10_000)
                / maxmemory
        })
        .unwrap_or_default();

    samples.extend([
        MetricSample::new(
            "redis_runtime_connected_clients",
            "Number of clients currently connected to Redis.",
            MetricKind::Gauge,
            snapshot.connected_clients.unwrap_or_default(),
        )
        .with_labels(labels.clone()),
        MetricSample::new(
            "redis_runtime_blocked_clients",
            "Number of Redis clients currently blocked by blocking commands.",
            MetricKind::Gauge,
            snapshot.blocked_clients.unwrap_or_default(),
        )
        .with_labels(labels.clone()),
        MetricSample::new(
            "redis_runtime_total_connections_received",
            "Total number of Redis connections received by the server.",
            MetricKind::Counter,
            snapshot.total_connections_received.unwrap_or_default(),
        )
        .with_labels(labels.clone()),
        MetricSample::new(
            "redis_runtime_rejected_connections_total",
            "Total number of Redis connections rejected by maxclients.",
            MetricKind::Counter,
            snapshot.rejected_connections.unwrap_or_default(),
        )
        .with_labels(labels.clone()),
        MetricSample::new(
            "redis_runtime_total_commands_processed",
            "Total Redis commands processed by the server.",
            MetricKind::Counter,
            snapshot.total_commands_processed.unwrap_or_default(),
        )
        .with_labels(labels.clone()),
        MetricSample::new(
            "redis_runtime_instantaneous_ops_per_sec",
            "Redis instantaneous operations per second.",
            MetricKind::Gauge,
            snapshot.instantaneous_ops_per_sec.unwrap_or_default(),
        )
        .with_labels(labels.clone()),
        MetricSample::new(
            "redis_runtime_total_error_replies",
            "Total Redis error replies returned by the server.",
            MetricKind::Counter,
            snapshot.total_error_replies.unwrap_or_default(),
        )
        .with_labels(labels.clone()),
        MetricSample::new(
            "redis_runtime_expired_keys_total",
            "Total number of Redis keys expired by the server.",
            MetricKind::Counter,
            snapshot.expired_keys.unwrap_or_default(),
        )
        .with_labels(labels.clone()),
        MetricSample::new(
            "redis_runtime_evicted_keys_total",
            "Total number of Redis keys evicted by maxmemory policy.",
            MetricKind::Counter,
            snapshot.evicted_keys.unwrap_or_default(),
        )
        .with_labels(labels.clone()),
        MetricSample::new(
            "redis_runtime_keyspace_hits_total",
            "Total number of Redis keyspace hits.",
            MetricKind::Counter,
            snapshot.keyspace_hits.unwrap_or_default(),
        )
        .with_labels(labels.clone()),
        MetricSample::new(
            "redis_runtime_keyspace_misses_total",
            "Total number of Redis keyspace misses.",
            MetricKind::Counter,
            snapshot.keyspace_misses.unwrap_or_default(),
        )
        .with_labels(labels.clone()),
        MetricSample::new(
            "redis_runtime_keyspace_hit_rate_basis_points",
            "Redis keyspace hit rate in basis points, where 10000 means 100 percent.",
            MetricKind::Gauge,
            hit_rate_basis_points,
        )
        .with_labels(labels.clone()),
        MetricSample::new(
            "redis_runtime_used_memory_bytes",
            "Redis used memory in bytes.",
            MetricKind::Gauge,
            snapshot.used_memory_bytes.unwrap_or_default(),
        )
        .with_labels(labels.clone()),
        MetricSample::new(
            "redis_runtime_maxmemory_bytes",
            "Redis configured maxmemory in bytes, or 0 when unlimited.",
            MetricKind::Gauge,
            snapshot.maxmemory_bytes.unwrap_or_default(),
        )
        .with_labels(labels.clone()),
        MetricSample::new(
            "redis_runtime_memory_usage_basis_points",
            "Redis memory usage relative to maxmemory in basis points, where 10000 means 100 percent; 0 when maxmemory is unlimited.",
            MetricKind::Gauge,
            memory_usage_basis_points,
        )
        .with_labels(labels.clone()),
        MetricSample::new(
            "redis_runtime_memory_fragmentation_ratio_basis_points",
            "Redis memory fragmentation ratio scaled by 10000.",
            MetricKind::Gauge,
            snapshot
                .memory_fragmentation_ratio_basis_points
                .unwrap_or_default(),
        )
        .with_labels(labels.clone()),
    ]);

    for lane in &snapshot.lanes {
        let lane_labels = vec![
            MetricLabel::new("backend", "redis"),
            MetricLabel::new("lane", lane.lane),
        ];
        samples.push(
            MetricSample::new(
                "redis_runtime_lane_command_errors_total",
                "Total Redis runtime command errors by connection lane.",
                MetricKind::Counter,
                lane.command_errors,
            )
            .with_labels(lane_labels.clone()),
        );
        samples.push(
            MetricSample::new(
                "redis_runtime_lane_command_count_total",
                "Total Redis runtime commands observed by connection lane.",
                MetricKind::Counter,
                lane.command_count,
            )
            .with_labels(lane_labels.clone()),
        );
        samples.push(
            MetricSample::new(
                "redis_runtime_lane_command_latency_ms_sum",
                "Cumulative Redis runtime command latency in milliseconds by connection lane.",
                MetricKind::Counter,
                lane.command_latency_total_ms,
            )
            .with_labels(lane_labels.clone()),
        );
        samples.push(
            MetricSample::new(
                "redis_runtime_lane_command_latency_ms_count",
                "Total Redis runtime command latency observations by connection lane.",
                MetricKind::Counter,
                lane.command_count,
            )
            .with_labels(lane_labels.clone()),
        );
        samples.push(
            MetricSample::new(
                "redis_runtime_lane_command_latency_ms_max",
                "Maximum Redis runtime command latency in milliseconds by connection lane since process start.",
                MetricKind::Gauge,
                lane.command_latency_max_ms,
            )
            .with_labels(lane_labels.clone()),
        );
        for bucket in &lane.command_latency_buckets {
            let mut bucket_labels = lane_labels.clone();
            bucket_labels.push(MetricLabel::new(
                "le",
                bucket
                    .le_ms
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "+Inf".to_string()),
            ));
            samples.push(
                MetricSample::new(
                    "redis_runtime_lane_command_latency_ms_bucket",
                    "Cumulative Redis runtime command latency histogram bucket by connection lane.",
                    MetricKind::Counter,
                    bucket.count,
                )
                .with_labels(bucket_labels),
            );
        }
        samples.push(
            MetricSample::new(
                "redis_runtime_lane_command_timeouts_total",
                "Total Redis runtime command timeouts by connection lane.",
                MetricKind::Counter,
                lane.command_timeouts,
            )
            .with_labels(lane_labels),
        );
    }

    samples
}

fn redis_runtime_zero_metric_samples(labels: Vec<MetricLabel>) -> Vec<MetricSample> {
    vec![
        MetricSample::new(
            "redis_runtime_connected_clients",
            "Number of clients currently connected to Redis.",
            MetricKind::Gauge,
            0,
        )
        .with_labels(labels.clone()),
        MetricSample::new(
            "redis_runtime_blocked_clients",
            "Number of Redis clients currently blocked by blocking commands.",
            MetricKind::Gauge,
            0,
        )
        .with_labels(labels.clone()),
        MetricSample::new(
            "redis_runtime_used_memory_bytes",
            "Redis used memory in bytes.",
            MetricKind::Gauge,
            0,
        )
        .with_labels(labels.clone()),
        MetricSample::new(
            "redis_runtime_memory_usage_basis_points",
            "Redis memory usage relative to maxmemory in basis points, where 10000 means 100 percent; 0 when maxmemory is unlimited.",
            MetricKind::Gauge,
            0,
        )
        .with_labels(labels),
    ]
}

fn usage_runtime_metric_samples(
    snapshot: &usage::UsageRuntimeMetricsSnapshot,
) -> Vec<MetricSample> {
    vec![
        MetricSample::new(
            "usage_runtime_enabled",
            "Whether the gateway usage runtime is enabled.",
            MetricKind::Gauge,
            u64::from(snapshot.enabled),
        ),
        MetricSample::new(
            "usage_runtime_queue_terminal_events_enabled",
            "Whether terminal usage events are queued before settlement.",
            MetricKind::Gauge,
            u64::from(snapshot.queue_terminal_events),
        ),
        MetricSample::new(
            "usage_runtime_queue_lifecycle_events_enabled",
            "Whether lifecycle usage events are queued.",
            MetricKind::Gauge,
            u64::from(snapshot.queue_lifecycle_events),
        ),
        MetricSample::new(
            "usage_runtime_queue_worker_count",
            "Minimum configured number of usage queue worker consumers.",
            MetricKind::Gauge,
            snapshot.worker_count as u64,
        ),
        MetricSample::new(
            "usage_runtime_queue_worker_autoscale_enabled",
            "Whether usage queue worker autoscaling is enabled.",
            MetricKind::Gauge,
            u64::from(snapshot.worker_autoscale_enabled),
        ),
        MetricSample::new(
            "usage_runtime_queue_worker_max_count",
            "Maximum configured number of elastic usage queue worker consumers.",
            MetricKind::Gauge,
            snapshot.worker_max_count as u64,
        ),
        MetricSample::new(
            "usage_runtime_queue_worker_record_concurrency_limit",
            "Maximum concurrent usage queue worker record writes; zero means unlimited.",
            MetricKind::Gauge,
            snapshot.worker_record_concurrency_limit.unwrap_or_default() as u64,
        ),
        MetricSample::new(
            "usage_runtime_queue_worker_record_concurrency_in_flight",
            "Current usage queue worker record writes in flight.",
            MetricKind::Gauge,
            snapshot.worker_record_concurrency_in_flight as u64,
        ),
        MetricSample::new(
            "usage_runtime_queue_worker_record_concurrency_max_in_flight",
            "Maximum observed usage queue worker record writes in flight.",
            MetricKind::Gauge,
            snapshot.worker_record_concurrency_max_in_flight as u64,
        ),
        MetricSample::new(
            "usage_runtime_queue_worker_record_concurrency_wait_total",
            "Total usage queue worker record writes that had to wait for the record concurrency gate.",
            MetricKind::Counter,
            snapshot.worker_record_concurrency_wait_total,
        ),
        MetricSample::new(
            "usage_runtime_queue_worker_record_deferred_total",
            "Total usage queue worker record writes deferred briefly because the database pool was under foreground pressure.",
            MetricKind::Counter,
            snapshot.worker_record_deferred_total,
        ),
        MetricSample::new(
            "usage_runtime_queue_worker_active_count",
            "Current active usage queue worker consumers managed by the supervisor.",
            MetricKind::Gauge,
            snapshot.worker_active_count as u64,
        ),
        MetricSample::new(
            "usage_runtime_queue_worker_desired_count",
            "Current desired usage queue worker consumers selected by autoscaling.",
            MetricKind::Gauge,
            snapshot.worker_desired_count as u64,
        ),
        MetricSample::new(
            "usage_runtime_queue_worker_read_batches_total",
            "Total successful usage queue read batches observed by supervised workers.",
            MetricKind::Counter,
            snapshot.worker_read_batches_total,
        ),
        MetricSample::new(
            "usage_runtime_queue_worker_read_entries_total",
            "Total usage queue entries read by supervised workers.",
            MetricKind::Counter,
            snapshot.worker_read_entries_total,
        ),
        MetricSample::new(
            "usage_runtime_queue_worker_reclaimed_entries_total",
            "Total stale usage queue entries reclaimed by supervised workers.",
            MetricKind::Counter,
            snapshot.worker_reclaimed_entries_total,
        ),
        MetricSample::new(
            "usage_runtime_queue_worker_acked_entries_total",
            "Total usage queue entries acknowledged and deleted by supervised workers.",
            MetricKind::Counter,
            snapshot.worker_acked_entries_total,
        ),
        MetricSample::new(
            "usage_runtime_queue_worker_dead_lettered_entries_total",
            "Total usage queue entries moved to dead letter by supervised workers.",
            MetricKind::Counter,
            snapshot.worker_dead_lettered_entries_total,
        ),
        MetricSample::new(
            "usage_runtime_queue_worker_process_failures_total",
            "Total usage queue processing failures observed by supervised workers.",
            MetricKind::Counter,
            snapshot.worker_process_failures_total,
        ),
        MetricSample::new(
            "usage_runtime_queue_worker_read_failures_total",
            "Total usage queue read failures observed by supervised workers.",
            MetricKind::Counter,
            snapshot.worker_read_failures_total,
        ),
        MetricSample::new(
            "usage_runtime_queue_worker_reclaim_failures_total",
            "Total usage queue reclaim failures observed by supervised workers.",
            MetricKind::Counter,
            snapshot.worker_reclaim_failures_total,
        ),
        MetricSample::new(
            "usage_runtime_retry_deferred_lifecycle_events_enabled",
            "Whether deferred lifecycle usage events are scheduled for local enqueue retry.",
            MetricKind::Gauge,
            u64::from(snapshot.retry_deferred_lifecycle_events),
        ),
        MetricSample::new(
            "usage_runtime_terminal_submission_limit",
            "Maximum concurrent end-to-end terminal usage submissions.",
            MetricKind::Gauge,
            snapshot.terminal_submission_limit as u64,
        ),
        MetricSample::new(
            "usage_runtime_terminal_submission_in_flight",
            "Current end-to-end terminal usage submissions in flight.",
            MetricKind::Gauge,
            snapshot.terminal_submission_in_flight as u64,
        ),
        MetricSample::new(
            "usage_runtime_terminal_submission_max_in_flight",
            "Maximum observed concurrent end-to-end terminal usage submissions.",
            MetricKind::Gauge,
            snapshot.terminal_submission_max_in_flight as u64,
        ),
        MetricSample::new(
            "usage_runtime_terminal_submission_rejected_total",
            "Total terminal usage submissions rejected by bounded ingress admission.",
            MetricKind::Counter,
            snapshot.terminal_submission_rejected_total,
        ),
        MetricSample::new(
            "usage_runtime_terminal_enqueue_in_flight",
            "Current terminal usage enqueue operations in flight.",
            MetricKind::Gauge,
            snapshot.terminal_enqueue_in_flight,
        ),
        MetricSample::new(
            "usage_runtime_terminal_enqueue_deferred_total",
            "Total terminal usage enqueue operations deferred after Redis failure, circuit open, or in-flight saturation.",
            MetricKind::Counter,
            snapshot.terminal_enqueue_deferred_total,
        ),
        MetricSample::new(
            "usage_runtime_terminal_enqueue_deferred_direct_write_total",
            "Total deferred terminal usage events persisted through bounded direct database fallback.",
            MetricKind::Counter,
            snapshot.terminal_enqueue_deferred_direct_write_total,
        ),
        MetricSample::new(
            "usage_runtime_terminal_enqueue_deferred_dropped_total",
            "Total deferred terminal usage events dropped after all bounded fallback capacity was exhausted.",
            MetricKind::Counter,
            snapshot.terminal_enqueue_deferred_dropped_total,
        ),
        MetricSample::new(
            "usage_runtime_terminal_enqueue_deferred_retry_total",
            "Total deferred terminal usage events scheduled for local retry.",
            MetricKind::Counter,
            snapshot.terminal_enqueue_deferred_retry_total,
        ),
        MetricSample::new(
            "usage_runtime_terminal_enqueue_failed_total",
            "Total terminal usage enqueue failures that opened the terminal enqueue circuit.",
            MetricKind::Counter,
            snapshot.terminal_enqueue_failed_total,
        ),
        MetricSample::new(
            "usage_runtime_terminal_direct_fallback_limit",
            "Maximum concurrent terminal direct database fallback operations.",
            MetricKind::Gauge,
            snapshot.terminal_direct_fallback_limit as u64,
        ),
        MetricSample::new(
            "usage_runtime_terminal_direct_fallback_in_flight",
            "Current terminal direct database fallback operations in flight.",
            MetricKind::Gauge,
            snapshot.terminal_direct_fallback_in_flight as u64,
        ),
        MetricSample::new(
            "usage_runtime_terminal_direct_fallback_max_in_flight",
            "Maximum observed concurrent terminal direct database fallback operations.",
            MetricKind::Gauge,
            snapshot.terminal_direct_fallback_max_in_flight as u64,
        ),
        MetricSample::new(
            "usage_runtime_terminal_direct_fallback_succeeded_total",
            "Total terminal events persisted through direct database fallback.",
            MetricKind::Counter,
            snapshot.terminal_direct_fallback_succeeded_total,
        ),
        MetricSample::new(
            "usage_runtime_terminal_direct_fallback_failed_total",
            "Total terminal direct database fallback operations that failed.",
            MetricKind::Counter,
            snapshot.terminal_direct_fallback_failed_total,
        ),
        MetricSample::new(
            "usage_runtime_terminal_direct_fallback_rejected_total",
            "Total terminal direct database fallback operations rejected because the writer was unavailable, pressured, or saturated.",
            MetricKind::Counter,
            snapshot.terminal_direct_fallback_rejected_total,
        ),
        MetricSample::new(
            "usage_runtime_lifecycle_enqueue_in_flight",
            "Current lifecycle usage enqueue operations in flight.",
            MetricKind::Gauge,
            snapshot.lifecycle_enqueue_in_flight,
        ),
        MetricSample::new(
            "usage_runtime_lifecycle_enqueue_deferred_total",
            "Total lifecycle usage enqueue operations deferred by circuit or in-flight limits.",
            MetricKind::Counter,
            snapshot.lifecycle_enqueue_deferred_total,
        ),
        MetricSample::new(
            "usage_runtime_lifecycle_enqueue_deferred_dropped_total",
            "Total deferred lifecycle usage events dropped instead of retrying.",
            MetricKind::Counter,
            snapshot.lifecycle_enqueue_deferred_dropped_total,
        ),
        MetricSample::new(
            "usage_runtime_lifecycle_enqueue_deferred_retry_total",
            "Total deferred lifecycle usage events scheduled for local retry.",
            MetricKind::Counter,
            snapshot.lifecycle_enqueue_deferred_retry_total,
        ),
        MetricSample::new(
            "usage_runtime_lifecycle_enqueue_failed_total",
            "Total lifecycle usage enqueue failures that opened the lifecycle enqueue circuit.",
            MetricKind::Counter,
            snapshot.lifecycle_enqueue_failed_total,
        ),
        MetricSample::new(
            "usage_runtime_enqueue_retry_scheduled_total",
            "Total usage events scheduled into the local enqueue dispatcher, including primary terminal events and retries.",
            MetricKind::Counter,
            snapshot.enqueue_retry_scheduled_total,
        ),
        MetricSample::new(
            "usage_runtime_enqueue_retry_recovered_total",
            "Total usage events successfully appended by the local enqueue dispatcher.",
            MetricKind::Counter,
            snapshot.enqueue_retry_recovered_total,
        ),
        MetricSample::new(
            "usage_runtime_enqueue_retry_pending",
            "Current usage events waiting or retrying in the local enqueue dispatcher.",
            MetricKind::Gauge,
            snapshot.enqueue_retry_pending,
        ),
        MetricSample::new(
            "usage_runtime_enqueue_retry_failed_total",
            "Total Redis queue append attempts failed inside the local enqueue dispatcher.",
            MetricKind::Counter,
            snapshot.enqueue_retry_failed_total,
        ),
        MetricSample::new(
            "usage_runtime_enqueue_retry_closed_or_unavailable_total",
            "Total usage events rejected because the local enqueue dispatcher was full, closed, or unavailable.",
            MetricKind::Counter,
            snapshot.enqueue_retry_closed_or_unavailable_total,
        ),
    ]
}

fn usage_queue_health_metric_samples(
    snapshot: &usage::UsageQueueHealthSnapshot,
) -> Vec<MetricSample> {
    let labels = vec![
        MetricLabel::new("stream", snapshot.stream_key.clone()),
        MetricLabel::new("group", snapshot.consumer_group.clone()),
    ];
    let dlq_labels = vec![MetricLabel::new("stream", snapshot.dlq_stream_key.clone())];
    vec![
        MetricSample::new(
            "usage_queue_health_unavailable",
            "Whether usage runtime queue health could not be read for this scrape.",
            MetricKind::Gauge,
            0,
        ),
        MetricSample::new(
            "usage_queue_enabled",
            "Whether the usage runtime queue is enabled.",
            MetricKind::Gauge,
            u64::from(snapshot.enabled),
        )
        .with_labels(labels.clone()),
        MetricSample::new(
            "usage_queue_configured",
            "Whether a runtime queue backend is configured for usage workers.",
            MetricKind::Gauge,
            u64::from(snapshot.configured),
        )
        .with_labels(labels.clone()),
        MetricSample::new(
            "usage_queue_stream_length",
            "Current number of entries retained in the usage runtime stream.",
            MetricKind::Gauge,
            snapshot.stream_length,
        )
        .with_labels(labels.clone()),
        MetricSample::new(
            "usage_queue_group_pending",
            "Current number of usage runtime stream entries pending acknowledgement.",
            MetricKind::Gauge,
            snapshot.group_pending,
        )
        .with_labels(labels.clone()),
        MetricSample::new(
            "usage_queue_group_lag",
            "Current number of usage runtime stream entries not yet delivered to the consumer group.",
            MetricKind::Gauge,
            snapshot.group_lag.unwrap_or_default(),
        )
        .with_labels(labels.clone()),
        MetricSample::new(
            "usage_queue_oldest_pending_idle_ms",
            "Idle milliseconds for the oldest pending usage runtime stream entry.",
            MetricKind::Gauge,
            snapshot.oldest_pending_idle_ms.unwrap_or_default(),
        )
        .with_labels(labels),
        MetricSample::new(
            "usage_queue_dlq_length",
            "Current number of retained usage runtime dead-letter stream entries.",
            MetricKind::Gauge,
            snapshot.dlq_length,
        )
        .with_labels(dlq_labels),
    ]
}

async fn usage_queue_health_metric_samples_with_timeout<F, E>(
    timeout: Duration,
    future: F,
) -> Vec<MetricSample>
where
    F: std::future::Future<Output = Result<usage::UsageQueueHealthSnapshot, E>>,
{
    match tokio::time::timeout(timeout, future).await {
        Ok(Ok(snapshot)) => usage_queue_health_metric_samples(&snapshot),
        Ok(Err(_)) | Err(_) => vec![MetricSample::new(
            "usage_queue_health_unavailable",
            "Whether usage runtime queue health could not be read for this scrape.",
            MetricKind::Gauge,
            1,
        )],
    }
}

fn usage_counter_pending_health_metric_samples(
    snapshot: &UsageCounterPendingHealthSnapshot,
    now_unix_secs: u64,
) -> Vec<MetricSample> {
    let oldest_pending_age_secs = snapshot
        .oldest_pending_created_at_unix_secs
        .filter(|_| snapshot.pending_rows > 0)
        .map(|created_at| now_unix_secs.saturating_sub(created_at))
        .unwrap_or_default();
    let mut samples = vec![
        MetricSample::new(
            "usage_counter_health_unavailable",
            "Whether usage counter outbox health could not be read for this scrape.",
            MetricKind::Gauge,
            0,
        ),
        MetricSample::new(
            "usage_counter_outbox_pending_rows",
            "Number of usage counter outbox rows waiting to be flushed.",
            MetricKind::Gauge,
            snapshot.pending_rows,
        ),
        MetricSample::new(
            "usage_counter_outbox_oldest_pending_age_seconds",
            "Age of the oldest pending usage counter outbox row in seconds.",
            MetricKind::Gauge,
            oldest_pending_age_secs,
        ),
        MetricSample::new(
            "usage_counter_outbox_oldest_pending_created_at_unix_secs",
            "Unix timestamp of the oldest pending usage counter outbox row.",
            MetricKind::Gauge,
            snapshot
                .oldest_pending_created_at_unix_secs
                .filter(|_| snapshot.pending_rows > 0)
                .unwrap_or_default(),
        ),
    ];
    for (kind, pending_rows) in &snapshot.pending_by_kind {
        samples.push(
            MetricSample::new(
                "usage_counter_outbox_pending_rows_by_kind",
                "Number of pending usage counter outbox rows by counter kind.",
                MetricKind::Gauge,
                *pending_rows,
            )
            .with_labels(vec![MetricLabel::new("kind", kind.clone())]),
        );
    }
    samples
}

fn usage_counter_exact_health_metric_samples(
    snapshot: &UsageCounterHealthSnapshot,
    snapshot_age_secs: u64,
) -> Vec<MetricSample> {
    vec![
        MetricSample::new(
            "usage_counter_outbox_processed_rows",
            "Number of usage counter outbox rows already processed, refreshed at low frequency.",
            MetricKind::Gauge,
            snapshot.processed_rows,
        ),
        MetricSample::new(
            "usage_counter_outbox_latest_processed_at_unix_secs",
            "Unix timestamp of the latest processed usage counter outbox row, refreshed at low frequency.",
            MetricKind::Gauge,
            snapshot.latest_processed_at_unix_secs.unwrap_or_default(),
        ),
        MetricSample::new(
            "usage_counter_exact_health_snapshot_age_seconds",
            "Age in seconds of the low-frequency exact usage counter health snapshot.",
            MetricKind::Gauge,
            snapshot_age_secs,
        ),
    ]
}

async fn usage_counter_pending_health_metric_samples_with_timeout<F, E>(
    timeout: Duration,
    future: F,
    now_unix_secs: u64,
) -> Vec<MetricSample>
where
    F: std::future::Future<Output = Result<UsageCounterPendingHealthSnapshot, E>>,
{
    match tokio::time::timeout(timeout, future).await {
        Ok(Ok(snapshot)) => usage_counter_pending_health_metric_samples(&snapshot, now_unix_secs),
        Ok(Err(_)) | Err(_) => vec![MetricSample::new(
            "usage_counter_health_unavailable",
            "Whether usage counter outbox health could not be read for this scrape.",
            MetricKind::Gauge,
            1,
        )],
    }
}

fn should_preserve_runtime_miss_diagnostic(
    existing: &LocalExecutionRuntimeMissDiagnostic,
    next: &LocalExecutionRuntimeMissDiagnostic,
) -> bool {
    runtime_miss_diagnostic_has_candidate_signal(existing)
        && !runtime_miss_diagnostic_has_candidate_signal(next)
}

fn runtime_miss_diagnostic_has_candidate_signal(
    diagnostic: &LocalExecutionRuntimeMissDiagnostic,
) -> bool {
    diagnostic.candidate_count.unwrap_or(0) > 0
        || diagnostic.skipped_candidate_count.unwrap_or(0) > 0
        || !diagnostic.skip_reasons.is_empty()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
    use aether_data::{DatabaseDriver, SqlDatabaseConfig, SqlPoolConfig};
    use aether_data_contracts::repository::usage::UsageCounterPendingHealthSnapshot;
    use serde_json::json;

    use super::{
        database_bounded_auth_load_limit, usage_counter_pending_health_metric_samples_with_timeout,
        usage_queue_health_metric_samples_with_timeout, AppState, MetricKind, MetricSample,
        METRIC_SNAPSHOT_TTL,
    };
    use crate::cache::SchedulerAffinityTarget;
    use crate::data::{GatewayDataConfig, GatewayDataState};

    #[test]
    fn auth_load_gate_reserves_half_of_foreground_database_pool() {
        assert_eq!(
            database_bounded_auth_load_limit(Some(192), Some(92)),
            Some(46)
        );
        assert_eq!(database_bounded_auth_load_limit(Some(4), Some(92)), Some(4));
        assert_eq!(database_bounded_auth_load_limit(Some(64), Some(1)), Some(1));
        assert_eq!(database_bounded_auth_load_limit(None, Some(92)), None);
        assert_eq!(database_bounded_auth_load_limit(Some(64), None), Some(64));
    }

    #[tokio::test]
    async fn runtime_pool_can_disable_background_isolation() {
        let config = GatewayDataConfig::from_database_config(
            SqlDatabaseConfig::new(
                DatabaseDriver::Postgres,
                "postgres://localhost/aether",
                SqlPoolConfig {
                    min_connections: 4,
                    max_connections: 20,
                    ..SqlPoolConfig::default()
                },
            )
            .expect("database config should be valid"),
        );

        let state = AppState::new()
            .expect("app state should build")
            .with_data_config_and_background_isolation(config, false)
            .expect("data state should build");

        assert!(!state.background_data_isolated);
    }

    #[tokio::test]
    async fn system_config_reads_use_short_lived_cache_until_app_invalidation() {
        let state = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled()
                    .with_system_config_values_for_tests([("site_name".to_string(), json!("old"))]),
            );

        assert_eq!(
            state
                .read_system_config_json_value("site_name")
                .await
                .expect("system config read should succeed"),
            Some(json!("old"))
        );

        state
            .data
            .upsert_system_config_value("site_name", &json!("bypassed"), None)
            .await
            .expect("direct data write should succeed");

        assert_eq!(
            state
                .read_system_config_json_value("site_name")
                .await
                .expect("cached system config read should succeed"),
            Some(json!("old"))
        );

        state
            .upsert_system_config_json_value("site_name", &json!("fresh"), None)
            .await
            .expect("app system config write should succeed");

        assert_eq!(
            state
                .read_system_config_json_value("site_name")
                .await
                .expect("refreshed system config read should succeed"),
            Some(json!("fresh"))
        );
    }

    #[tokio::test]
    async fn system_config_entry_write_refreshes_cache_and_scheduler_affinity_for_routing_keys() {
        let state = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled().with_system_config_values_for_tests([(
                    "keep_priority_on_conversion".to_string(),
                    json!(false),
                )]),
            );
        let cache_key = "scheduler_affinity:api-key-1:openai:chat:gpt-5";
        let ttl = std::time::Duration::from_secs(300);

        assert_eq!(
            state
                .read_system_config_json_value("keep_priority_on_conversion")
                .await
                .expect("system config read should succeed"),
            Some(json!(false))
        );
        state.remember_scheduler_affinity_target(
            cache_key,
            SchedulerAffinityTarget {
                provider_id: "provider-old".to_string(),
                endpoint_id: "endpoint-old".to_string(),
                key_id: "key-old".to_string(),
            },
            ttl,
            128,
        );
        assert!(state
            .read_scheduler_affinity_target(cache_key, ttl)
            .is_some());

        let initial_epoch = state.scheduler_affinity_epoch();
        state
            .upsert_system_config_entry("keep_priority_on_conversion", &json!(true), None)
            .await
            .expect("admin config write should succeed");

        assert_eq!(
            state
                .read_system_config_json_value("keep_priority_on_conversion")
                .await
                .expect("system config read should use refreshed cache"),
            Some(json!(true))
        );
        assert!(state.scheduler_affinity_epoch() > initial_epoch);
        assert_eq!(state.read_scheduler_affinity_target(cache_key, ttl), None);
    }

    #[tokio::test]
    async fn system_config_write_refreshes_frontdoor_rpm_default_cache() {
        let state = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled().with_system_config_values_for_tests([(
                    "rate_limit_per_minute".to_string(),
                    json!(1),
                )]),
            );

        assert_eq!(
            state
                .frontdoor_user_rpm()
                .current_system_default_limit(&state)
                .await
                .expect("default rpm limit should read"),
            1
        );
        state
            .upsert_system_config_entry("rate_limit_per_minute", &json!(0), None)
            .await
            .expect("rpm system config should update");

        assert_eq!(
            state
                .frontdoor_user_rpm()
                .current_system_default_limit(&state)
                .await
                .expect("default rpm limit should use refreshed value"),
            0
        );
    }

    #[tokio::test]
    async fn replacing_data_state_clears_system_config_cache() {
        let mut state = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled()
                    .with_system_config_values_for_tests([("site_name".to_string(), json!("old"))]),
            );

        assert_eq!(
            state
                .read_system_config_json_value("site_name")
                .await
                .expect("system config read should succeed"),
            Some(json!("old"))
        );

        state.replace_data_state(Arc::new(
            GatewayDataState::disabled()
                .with_system_config_values_for_tests([("site_name".to_string(), json!("new"))]),
        ));

        assert_eq!(
            state
                .read_system_config_json_value("site_name")
                .await
                .expect("system config read should reflect replaced data"),
            Some(json!("new"))
        );
    }

    #[tokio::test]
    async fn metric_samples_include_auth_snapshot_load_gate() {
        let state = AppState::new().expect("app state should build");
        assert!(state.prewarm_metric_snapshot().await);
        let samples = state.metric_samples().await;

        assert!(samples.iter().any(|sample| {
            sample.name == "concurrency_available_permits"
                && sample
                    .labels
                    .iter()
                    .any(|label| label.key == "gate" && label.value == "gateway_auth_snapshot_load")
        }));
    }

    #[tokio::test]
    async fn metric_samples_include_request_body_buffer_budget_usage() {
        let state = AppState::new().expect("app state should build");
        let _permit = Arc::clone(&state.request_body_buffer_budget)
            .acquire_many_owned(2)
            .await
            .expect("request body budget should be open");
        assert!(state.prewarm_metric_snapshot().await);
        let samples = state.metric_samples().await;

        assert!(samples.iter().any(|sample| {
            sample.name == "request_body_buffer_in_use_bytes"
                && sample.value
                    == u64::try_from(2 * crate::state::REQUEST_BODY_BUFFER_PERMIT_BYTES)
                        .unwrap_or(u64::MAX)
        }));
    }

    #[tokio::test]
    async fn metric_samples_reuse_recent_snapshot() {
        let state = AppState::new().expect("app state should build");
        assert!(state.prewarm_metric_snapshot().await);
        let first = state.metric_samples().await;
        let _permit = Arc::clone(&state.request_body_buffer_budget)
            .acquire_many_owned(2)
            .await
            .expect("request body budget should be open");
        let second = state.metric_samples().await;

        assert_eq!(first, second);
        assert!(state.metric_snapshot.read().await.is_some());
    }

    #[tokio::test]
    async fn metric_snapshot_prewarm_populates_low_frequency_exact_counter_metrics() {
        let state = AppState::new().expect("app state should build");

        assert!(state.prewarm_metric_snapshot().await);
        assert!(state
            .usage_counter_exact_health_metric_snapshot
            .read()
            .await
            .is_some());
        let samples = state.metric_samples().await;
        assert!(samples
            .iter()
            .any(|sample| sample.name == "usage_counter_outbox_processed_rows"));
        assert!(samples
            .iter()
            .any(|sample| sample.name == "usage_counter_outbox_latest_processed_at_unix_secs"));
        assert!(samples
            .iter()
            .any(|sample| sample.name == "usage_counter_exact_health_snapshot_age_seconds"));
    }

    #[tokio::test]
    async fn stale_metric_samples_return_immediately_while_background_refreshes() {
        let state = AppState::new().expect("app state should build");
        let stale_sample = MetricSample::new(
            "stale_metric_snapshot_test",
            "Test-only stale metric snapshot marker.",
            MetricKind::Gauge,
            1,
        );
        let stale_created_at = std::time::Instant::now()
            .checked_sub(METRIC_SNAPSHOT_TTL + Duration::from_millis(1))
            .expect("stale timestamp should be representable");
        *state.metric_snapshot.write().await = Some((stale_created_at, vec![stale_sample.clone()]));

        let returned = tokio::time::timeout(Duration::from_millis(100), state.metric_samples())
            .await
            .expect("stale scrape should return without awaiting refresh I/O");
        assert_eq!(returned, vec![stale_sample]);

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let refreshed =
                    state
                        .metric_snapshot
                        .read()
                        .await
                        .as_ref()
                        .is_some_and(|(_, samples)| {
                            samples
                                .iter()
                                .all(|sample| sample.name != "stale_metric_snapshot_test")
                        });
                if refreshed {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("background metric refresh should replace the stale snapshot");
    }

    #[tokio::test]
    async fn metric_samples_fail_open_while_initial_snapshot_refresh_is_in_progress() {
        let state = AppState::new().expect("app state should build");
        let _refresh_guard = state.metric_snapshot_refresh.lock().await;

        let samples = tokio::time::timeout(Duration::from_millis(100), state.metric_samples())
            .await
            .expect("contending scrape should not wait for the initial refresh");

        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].name, "service_up");
        assert_eq!(samples[0].value, 1);
        assert!(state.metric_snapshot.read().await.is_none());
    }

    #[tokio::test]
    async fn usage_queue_health_metrics_timeout_fails_open() {
        let samples = tokio::time::timeout(
            Duration::from_millis(100),
            usage_queue_health_metric_samples_with_timeout(
                Duration::from_millis(1),
                std::future::pending::<Result<crate::usage::UsageQueueHealthSnapshot, ()>>(),
            ),
        )
        .await
        .expect("metrics timeout wrapper should remain bounded");

        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].name, "usage_queue_health_unavailable");
        assert_eq!(samples[0].value, 1);
    }

    #[tokio::test]
    async fn usage_counter_health_metrics_timeout_fails_open() {
        let samples = tokio::time::timeout(
            Duration::from_millis(100),
            usage_counter_pending_health_metric_samples_with_timeout(
                Duration::from_millis(1),
                std::future::pending::<Result<UsageCounterPendingHealthSnapshot, ()>>(),
                1_000,
            ),
        )
        .await
        .expect("metrics timeout wrapper should remain bounded");

        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].name, "usage_counter_health_unavailable");
        assert_eq!(samples[0].value, 1);
    }

    #[test]
    fn background_worker_state_uses_isolated_data_pool() {
        let mut state = AppState::new().expect("app state should build");
        let foreground = Arc::new(GatewayDataState::disabled());
        let background = Arc::new(GatewayDataState::disabled());
        state.replace_data_states(foreground, background.clone(), true);

        let worker_state = state.background_worker_state();

        assert!(Arc::ptr_eq(&worker_state.data, &state.background_data));
        assert!(!Arc::ptr_eq(&worker_state.data, &state.data));
        assert!(worker_state.data.has_usage_worker_queue());
        assert!(state.background_data_isolated);
    }

    #[test]
    fn request_candidate_queue_writer_uses_background_data_only_when_isolated() {
        let foreground_repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let background_repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let foreground = Arc::new(
            GatewayDataState::with_request_candidate_repository_for_tests(foreground_repository),
        );
        let background = Arc::new(
            GatewayDataState::with_request_candidate_repository_for_tests(background_repository),
        );
        let mut state = AppState::new().expect("app state should build");

        state.replace_data_states(foreground, background, true);

        let selected = state.request_candidate_queue_data_state();
        assert!(Arc::ptr_eq(selected, &state.background_data));
        assert!(!Arc::ptr_eq(selected, &state.data));
        let selected_writer = selected
            .request_candidate_writer()
            .expect("isolated background writer should exist");
        let background_writer = state
            .background_data
            .request_candidate_writer()
            .expect("background writer should exist");
        let foreground_writer = state
            .data
            .request_candidate_writer()
            .expect("foreground writer should exist");
        assert!(Arc::ptr_eq(&selected_writer, &background_writer));
        assert!(!Arc::ptr_eq(&selected_writer, &foreground_writer));

        let shared_repository = Arc::new(InMemoryRequestCandidateRepository::default());
        state.replace_data_state(Arc::new(
            GatewayDataState::with_request_candidate_repository_for_tests(shared_repository),
        ));

        let selected = state.request_candidate_queue_data_state();
        assert!(Arc::ptr_eq(selected, &state.data));
        assert!(!state.background_data_isolated);
        let selected_writer = selected
            .request_candidate_writer()
            .expect("shared writer should exist");
        let foreground_writer = state
            .data
            .request_candidate_writer()
            .expect("shared foreground writer should exist");
        assert!(Arc::ptr_eq(&selected_writer, &foreground_writer));
    }

    #[test]
    fn replacing_shared_data_state_preserves_background_usage_queue() {
        let mut state = AppState::new().expect("app state should build");

        state.replace_data_state(Arc::new(GatewayDataState::disabled()));

        assert!(state.data.has_usage_worker_queue());
        assert!(state.background_data.has_usage_worker_queue());
        assert!(state
            .background_worker_state()
            .data
            .has_usage_worker_queue());
    }

    #[test]
    fn scheduler_affinity_epoch_blocks_stale_rewarm_after_invalidation() {
        let state = AppState::new().expect("app state should build");
        let cache_key = "scheduler_affinity:api-key-1:openai:chat:gpt-5";
        let ttl = std::time::Duration::from_secs(300);
        let first_target = crate::cache::SchedulerAffinityTarget {
            provider_id: "provider-old".to_string(),
            endpoint_id: "endpoint-old".to_string(),
            key_id: "key-old".to_string(),
        };
        let next_target = crate::cache::SchedulerAffinityTarget {
            provider_id: "provider-new".to_string(),
            endpoint_id: "endpoint-new".to_string(),
            key_id: "key-new".to_string(),
        };
        let initial_epoch = state.scheduler_affinity_epoch();

        assert!(state.remember_scheduler_affinity_target_for_epoch(
            cache_key,
            first_target.clone(),
            ttl,
            16,
            Some(initial_epoch),
        ));
        assert_eq!(
            state.read_scheduler_affinity_target(cache_key, ttl),
            Some(first_target)
        );

        let next_epoch = state.invalidate_scheduler_affinity_cache();

        assert!(!state.remember_scheduler_affinity_target_for_epoch(
            cache_key,
            next_target.clone(),
            ttl,
            16,
            Some(initial_epoch),
        ));
        assert_eq!(state.read_scheduler_affinity_target(cache_key, ttl), None);

        assert!(state.remember_scheduler_affinity_target_for_epoch(
            cache_key,
            next_target.clone(),
            ttl,
            16,
            Some(next_epoch),
        ));
        assert_eq!(
            state.read_scheduler_affinity_target(cache_key, ttl),
            Some(next_target)
        );
    }
}
