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
use aether_http::{build_http_client, HttpClientConfig};
use aether_runtime::{
    service_up_sample, AdmissionPermit, ConcurrencyGate, ConcurrencySnapshot, MetricKind,
    MetricLabel, MetricSample,
};
use aether_runtime_state::{
    MemoryRuntimeStateConfig, RuntimeQueueStore, RuntimeSemaphore, RuntimeSemaphoreError,
    RuntimeSemaphoreSnapshot, RuntimeState,
};
use aether_scheduler_core::PROVIDER_KEY_RPM_WINDOW_SECS;
use tracing::warn;

use super::{
    AppState, FrontdoorCorsConfig, FrontdoorRuntimeGuardConfig, LocalExecutionRuntimeMissDiagnostic,
};

use super::super::async_task::{
    spawn_video_task_poller, VideoTaskPollerConfig, VideoTaskService, VideoTaskTruthSourceMode,
};
use super::super::cache::{
    AuthApiKeyLastUsedCache, AuthContextCache, AuthSnapshotCache, DashboardResponseCache,
    DirectPlanBypassCache, JsonValueCache, SchedulerAffinityCache, SchedulerAffinitySnapshotEntry,
    SchedulerAffinityTarget, SystemConfigCache, ValueCache,
};
use super::super::codex_adapter::config::{
    parse_codex_adapter_runtime_config, CodexAdapterRuntimeConfig, CodexAdapterRuntimeConfigCache,
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
use crate::maintenance::spawn_gemini_file_mapping_cleanup_worker;
use crate::maintenance::spawn_oauth_token_refresh_worker;
use crate::maintenance::spawn_pending_cleanup_worker;
use crate::maintenance::spawn_pool_monitor_worker;
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

fn system_config_key_affects_codex_adapter(key: &str) -> bool {
    matches!(
        key.trim(),
        aether_admin::codex_adapter::CODEX_ADAPTER_ENABLED_CONFIG_KEY
            | aether_admin::codex_adapter::CODEX_ADAPTER_ROUTES_CONFIG_KEY
    )
}

fn codex_adapter_bad_request(message: String) -> GatewayError {
    GatewayError::Client {
        status: http::StatusCode::BAD_REQUEST,
        message,
    }
}

impl AppState {
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
        self.clear_provider_transport_snapshot_cache();
        self.invalidate_scheduler_affinity_cache();
        self.invalidate_auth_context_cache();
        self.system_config_cache.clear();
        self.codex_adapter_config_cache.clear();
        self.frontdoor_user_rpm.clear_system_default_cache();
        let data = Arc::new(
            (*data)
                .clone()
                .with_usage_worker_queue(Self::usage_worker_queue_for(&self.runtime_state)),
        );
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
        Ok(Self {
            #[cfg(test)]
            execution_runtime_override_base_url: execution_runtime_override_base_url
                .map(|value| value.trim_end_matches('/').to_string())
                .filter(|value| !value.is_empty()),
            #[cfg(test)]
            execution_runtime_sync_override: None,
            data: Arc::clone(&data),
            runtime_state: runtime_state.clone(),
            usage_runtime: Arc::new(usage::UsageRuntime::disabled()),
            video_tasks: Arc::new(VideoTaskService::new(
                VideoTaskTruthSourceMode::PythonSyncReport,
            )),
            video_task_poller: None,
            frontdoor_runtime_guards: Arc::new(FrontdoorRuntimeGuardConfig::from_env()),
            request_gate: None,
            distributed_request_gate: None,
            client,
            auth_context_cache: Arc::new(AuthContextCache::default()),
            auth_snapshot_cache: Arc::new(AuthSnapshotCache::default()),
            user_model_capability_settings_cache: Arc::new(JsonValueCache::default()),
            user_feature_settings_cache: Arc::new(JsonValueCache::default()),
            auth_api_key_force_capabilities_cache: Arc::new(JsonValueCache::default()),
            auth_api_key_feature_settings_cache: Arc::new(JsonValueCache::default()),
            provider_quota_snapshot_cache: Arc::new(ValueCache::default()),
            user_groups_for_user_cache: Arc::new(ValueCache::default()),
            auth_api_key_last_used_cache: Arc::new(AuthApiKeyLastUsedCache::default()),
            oauth_refresh: Arc::new(provider_transport::LocalOAuthRefreshCoordinator::new()),
            direct_plan_bypass_cache: Arc::new(DirectPlanBypassCache::default()),
            scheduler_affinity_cache: Arc::new(SchedulerAffinityCache::default()),
            scheduler_affinity_epoch: Arc::new(AtomicU64::new(0)),
            dashboard_response_cache: Arc::new(DashboardResponseCache::default()),
            system_config_cache: Arc::new(SystemConfigCache::default()),
            codex_adapter_config_cache: Arc::new(CodexAdapterRuntimeConfigCache::default()),
            fallback_metrics: Arc::new(fallback_metrics::GatewayFallbackMetrics::default()),
            request_candidate_queue: None,
            frontdoor_cors: None,
            frontdoor_user_rpm: Arc::new(FrontdoorUserRpmLimiter::new(
                FrontdoorUserRpmConfig::default(),
            )),
            tunnel: crate::tunnel::EmbeddedTunnelState::with_data_and_runtime_state(
                data,
                runtime_state.clone(),
            ),
            provider_transport_snapshot_cache: Arc::new(StdMutex::new(HashMap::new())),
            provider_key_rpm_resets: Arc::new(StdMutex::new(HashMap::new())),
            local_execution_runtime_miss_diagnostics: Arc::new(StdMutex::new(HashMap::new())),
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
        mut self,
        config: GatewayDataConfig,
    ) -> Result<Self, aether_data::DataLayerError> {
        self.replace_data_state(Arc::new(GatewayDataState::from_config(config)?));
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
        self.data = Arc::new(
            (*self.data)
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
                self.data
                    .request_candidate_writer()
                    .map(|writer| RequestCandidateQueueRuntime::spawn(writer, config))
            }
        } else {
            None
        };
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

        let _guard = self.system_config_cache.load_guard().await;
        if let Some(value) = self.system_config_cache.get(key, SYSTEM_CONFIG_CACHE_TTL) {
            return Ok(value);
        }

        let value = self
            .data
            .find_system_config_value(key)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        self.system_config_cache
            .insert(key.to_string(), value.clone(), SYSTEM_CONFIG_CACHE_TTL);
        Ok(value)
    }

    pub(crate) async fn read_codex_adapter_runtime_config(
        &self,
    ) -> Result<Arc<CodexAdapterRuntimeConfig>, GatewayError> {
        if let Some(config) = self.codex_adapter_config_cache.get() {
            return Ok(config);
        }

        let stored = self
            .read_system_config_json_value(
                aether_admin::codex_adapter::CODEX_ADAPTER_ROUTES_CONFIG_KEY,
            )
            .await?
            .unwrap_or(serde_json::Value::Null);
        let normalized =
            aether_admin::codex_adapter::normalize_codex_adapter_routes_config_value(stored)
                .map_err(codex_adapter_bad_request)?;
        let config = Arc::new(
            parse_codex_adapter_runtime_config(normalized).map_err(codex_adapter_bad_request)?,
        );
        self.codex_adapter_config_cache.store(Arc::clone(&config));
        Ok(config)
    }

    pub(crate) async fn system_config_bool_for_codex_adapter_enabled(
        &self,
    ) -> Result<bool, GatewayError> {
        Ok(self
            .read_system_config_json_value(
                aether_admin::codex_adapter::CODEX_ADAPTER_ENABLED_CONFIG_KEY,
            )
            .await?
            .and_then(|value| value.as_bool())
            .unwrap_or(false))
    }

    pub(crate) async fn upsert_system_config_json_value(
        &self,
        key: &str,
        value: &serde_json::Value,
        description: Option<&str>,
    ) -> Result<serde_json::Value, GatewayError> {
        crate::codex_adapter::compatibility_validation::validate_codex_adapter_routes_config_write(
            self, key, value,
        )
        .await?;
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
        crate::codex_adapter::compatibility_validation::validate_codex_adapter_routes_config_write(
            self, key, value,
        )
        .await?;
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
        if system_config_key_affects_codex_adapter(key) {
            self.codex_adapter_config_cache.clear();
        }
        Ok(deleted)
    }

    pub(crate) fn invalidate_provider_routing_caches(&self) {
        self.data.clear_minimal_candidate_selection_cache();
        self.data.clear_provider_catalog_cache();
        self.clear_provider_transport_snapshot_cache();
        self.invalidate_scheduler_affinity_cache();
    }

    pub(crate) fn invalidate_provider_health_routing_caches(&self) {
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
        self.provider_quota_snapshot_cache.clear();
        self.user_groups_for_user_cache.clear();
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
        if system_config_key_affects_codex_adapter(key) {
            self.codex_adapter_config_cache.clear();
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
            self.codex_adapter_config_cache.clear();
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

    pub(crate) async fn distributed_request_concurrency_snapshot(
        &self,
    ) -> Result<Option<RuntimeSemaphoreSnapshot>, RuntimeSemaphoreError> {
        match self.distributed_request_gate.as_ref() {
            Some(gate) => gate.snapshot().await.map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn metric_samples(&self) -> Vec<MetricSample> {
        let mut samples = vec![service_up_sample("aether-gateway")];
        if let Some(snapshot) = self.request_concurrency_snapshot() {
            samples.extend(snapshot.to_metric_samples("gateway_requests"));
        }
        if let Some(gate) = self.distributed_request_gate.as_ref() {
            match gate.snapshot().await {
                Ok(snapshot) => {
                    samples.extend(snapshot.to_metric_samples("gateway_requests_distributed"));
                }
                Err(_) => samples.push(
                    MetricSample::new(
                        "concurrency_unavailable",
                        "Whether the distributed concurrency gate is currently unavailable.",
                        MetricKind::Gauge,
                        1,
                    )
                    .with_labels(vec![MetricLabel::new(
                        "gate",
                        "gateway_requests_distributed",
                    )]),
                ),
            }
        }
        if let Some(summary) = self.data.database_pool_summary() {
            samples.extend(database_pool_metric_samples(&summary));
        }
        if let Some(queue) = self.request_candidate_queue.as_ref() {
            samples.extend(queue.metric_samples());
        }
        samples.extend(self.tunnel.metric_samples());
        samples.extend(self.fallback_metrics.metric_samples());
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
            .lock()
            .expect("local execution runtime miss diagnostics should lock")
            .remove(trace_id);
    }

    pub(crate) fn set_local_execution_runtime_miss_diagnostic(
        &self,
        trace_id: &str,
        diagnostic: LocalExecutionRuntimeMissDiagnostic,
    ) {
        let mut diagnostics = self
            .local_execution_runtime_miss_diagnostics
            .lock()
            .expect("local execution runtime miss diagnostics should lock");
        if diagnostics
            .get(trace_id)
            .is_some_and(|existing| should_preserve_runtime_miss_diagnostic(existing, &diagnostic))
        {
            return;
        }
        diagnostics.insert(trace_id.to_string(), diagnostic);
    }

    pub(crate) fn mutate_local_execution_runtime_miss_diagnostic<F>(
        &self,
        trace_id: &str,
        mutate: F,
    ) where
        F: FnOnce(&mut LocalExecutionRuntimeMissDiagnostic),
    {
        let mut diagnostics = self
            .local_execution_runtime_miss_diagnostics
            .lock()
            .expect("local execution runtime miss diagnostics should lock");
        if let Some(diagnostic) = diagnostics.get_mut(trace_id) {
            mutate(diagnostic);
        }
    }

    pub(crate) fn local_execution_runtime_miss_diagnostic_has_candidate_signal(
        &self,
        trace_id: &str,
    ) -> bool {
        self.local_execution_runtime_miss_diagnostics
            .lock()
            .expect("local execution runtime miss diagnostics should lock")
            .get(trace_id)
            .is_some_and(runtime_miss_diagnostic_has_candidate_signal)
    }

    pub(crate) fn take_local_execution_runtime_miss_diagnostic(
        &self,
        trace_id: &str,
    ) -> Option<LocalExecutionRuntimeMissDiagnostic> {
        self.local_execution_runtime_miss_diagnostics
            .lock()
            .expect("local execution runtime miss diagnostics should lock")
            .remove(trace_id)
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

    pub fn spawn_background_tasks(&self) -> crate::task_runtime::TaskSupervisor {
        let mut supervisor = crate::task_runtime::TaskSupervisor::new();
        let record_boot = |task_key: &'static str| {
            if !self.has_background_task_data_writer() {
                return;
            }
            let Some(definition) = crate::task_runtime::task_definition(task_key) else {
                return;
            };
            std::mem::drop(crate::task_runtime::spawn_record_worker_boot(
                self.clone(),
                task_key,
                crate::task_runtime::background_task_kind(definition.kind),
                definition.trigger,
            ));
        };
        let mut supervise_worker =
            |task_key: &'static str, handle: Option<tokio::task::JoinHandle<()>>| {
                if let Some(handle) = handle {
                    supervisor.supervise_handle(task_key, handle);
                    record_boot(task_key);
                }
            };

        supervise_worker(
            crate::task_runtime::TASK_KEY_USAGE_QUEUE_WORKER,
            self.usage_runtime.spawn_worker(self.data.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_USAGE_COUNTER_FLUSH,
            spawn_usage_counter_flush_worker(self.data.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_PROVIDER_QUOTA_RESET,
            crate::wallet_runtime::spawn_provider_quota_reset_worker(self.data.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_AUDIT_CLEANUP,
            spawn_audit_cleanup_worker(self.data.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_DB_MAINTENANCE,
            spawn_db_maintenance_worker(self.data.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_WALLET_DAILY_USAGE_AGG,
            spawn_wallet_daily_usage_aggregation_worker(self.data.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_STATS_DAILY_AGG,
            spawn_stats_aggregation_worker(self.data.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_USAGE_CLEANUP,
            spawn_usage_cleanup_worker(self.data.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_POOL_MONITOR,
            spawn_pool_monitor_worker(self.data.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_ACCOUNT_SELF_CHECK,
            spawn_account_self_check_worker(self.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_POOL_SCORE_REBUILD,
            spawn_pool_score_rebuild_worker(self.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_STATS_HOURLY_AGG,
            spawn_stats_hourly_aggregation_worker(self.data.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_PENDING_CLEANUP,
            spawn_pending_cleanup_worker(self.data.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_PROXY_NODE_STALE_CLEANUP,
            spawn_proxy_node_stale_cleanup_worker(self.data.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_PROXY_NODE_METRICS_CLEANUP,
            spawn_proxy_node_metrics_cleanup_worker(self.data.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_PROXY_UPGRADE_ROLLOUT,
            spawn_proxy_upgrade_rollout_worker(self.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_PROVIDER_CHECKIN,
            spawn_provider_checkin_worker(self.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_PROVIDER_QUOTA_ALERT,
            spawn_provider_quota_alert_worker(self.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_OAUTH_TOKEN_REFRESH,
            spawn_oauth_token_refresh_worker(self.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_REQUEST_CANDIDATE_CLEANUP,
            spawn_request_candidate_cleanup_worker(self.data.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_GEMINI_FILES_CLEANUP,
            spawn_gemini_file_mapping_cleanup_worker(self.data.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_MODEL_FETCH_WORKER,
            spawn_model_fetch_worker(self.clone()),
        );
        supervise_worker(
            crate::task_runtime::TASK_KEY_VIDEO_TASK_POLLER,
            spawn_video_task_poller(self.clone()),
        );
        supervise_worker(
            crate::backup::worker::S3_BACKUP_WORKER_TASK_KEY,
            crate::backup::worker::spawn_s3_backup_worker(self.clone()),
        );

        supervisor
    }
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

    use serde_json::json;

    use super::AppState;
    use crate::cache::SchedulerAffinityTarget;
    use crate::data::GatewayDataState;

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
    async fn codex_adapter_runtime_config_uses_cache_until_routes_write_invalidates() {
        let routes_key = aether_admin::codex_adapter::CODEX_ADAPTER_ROUTES_CONFIG_KEY;
        let old_routes = json!([{
            "codex_model": "gpt-5.5",
            "enabled": true,
            "scheduling_mode": "priority",
            "candidates": [{
                "global_model": "glm-4.6",
                "enabled": true,
                "priority": 0,
                "weight": 1
            }]
        }]);
        let new_routes = json!([{
            "codex_model": "gpt-5.6",
            "enabled": true,
            "scheduling_mode": "priority",
            "candidates": [{
                "global_model": "deepseek-v3.2",
                "enabled": true,
                "priority": 0,
                "weight": 1
            }]
        }]);
        let state = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled()
                    .with_system_config_values_for_tests([(routes_key.to_string(), old_routes)]),
            );

        let cached = state
            .read_codex_adapter_runtime_config()
            .await
            .expect("codex adapter config should read");
        assert_eq!(cached.routes[0].codex_model, "gpt-5.5");

        state
            .data
            .upsert_system_config_value(routes_key, &new_routes, None)
            .await
            .expect("direct data write should succeed");
        let still_cached = state
            .read_codex_adapter_runtime_config()
            .await
            .expect("codex adapter config should use parsed cache");
        assert_eq!(still_cached.routes[0].codex_model, "gpt-5.5");

        state
            .upsert_system_config_entry(routes_key, &new_routes, None)
            .await
            .expect("app config write should succeed");
        let refreshed = state
            .read_codex_adapter_runtime_config()
            .await
            .expect("codex adapter config should refresh");
        assert_eq!(refreshed.routes[0].codex_model, "gpt-5.6");

        state
            .delete_system_config_value(routes_key)
            .await
            .expect("app config delete should succeed");
        let deleted = state
            .read_codex_adapter_runtime_config()
            .await
            .expect("deleted codex adapter config should default empty");
        assert!(deleted.routes.is_empty());
    }

    #[tokio::test]
    async fn codex_adapter_enabled_helper_reads_true_only() {
        let enabled_key = aether_admin::codex_adapter::CODEX_ADAPTER_ENABLED_CONFIG_KEY;
        let state = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled()
                    .with_system_config_values_for_tests([(enabled_key.to_string(), json!(true))]),
            );

        assert!(state
            .system_config_bool_for_codex_adapter_enabled()
            .await
            .expect("enabled flag should read"));

        state
            .upsert_system_config_entry(enabled_key, &json!("true"), None)
            .await
            .expect("app config write should succeed");

        assert!(!state
            .system_config_bool_for_codex_adapter_enabled()
            .await
            .expect("non-bool enabled flag should read as false"));
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
