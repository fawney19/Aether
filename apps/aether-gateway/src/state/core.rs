use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use aether_data::repository::proxy_nodes::{
    ProxyNodeHeartbeatMutation, ProxyNodeManualCreateMutation, ProxyNodeManualUpdateMutation,
    ProxyNodeTunnelStatusMutation, StoredProxyNode, StoredProxyNodeEvent,
};
use aether_http::{build_http_client, HttpClientConfig};
use aether_runtime::{
    service_up_sample, AdmissionPermit, ConcurrencyGate, ConcurrencySnapshot,
    DistributedConcurrencyError, DistributedConcurrencyGate, DistributedConcurrencySnapshot,
    MetricKind, MetricLabel, MetricSample,
};
use aether_scheduler_core::PROVIDER_KEY_RPM_WINDOW_SECS;
use tokio::task::JoinHandle;

use super::{AppState, FrontdoorCorsConfig, LocalExecutionRuntimeMissDiagnostic};

use super::super::async_task::{
    spawn_video_task_poller, VideoTaskPollerConfig, VideoTaskService, VideoTaskTruthSourceMode,
};
use super::super::cache::{
    AuthApiKeyLastUsedCache, AuthContextCache, DashboardResponseCache, DirectPlanBypassCache,
    SchedulerAffinityCache, SchedulerAffinitySnapshotEntry, SchedulerAffinityTarget,
    SystemConfigCache,
};
use super::super::data::{GatewayDataConfig, GatewayDataState};
use super::super::fallback_metrics;
use super::super::fallback_metrics::{GatewayFallbackMetricKind, GatewayFallbackReason};
use super::super::model_fetch::spawn_model_fetch_worker;
use super::super::rate_limit::{FrontdoorUserRpmConfig, FrontdoorUserRpmLimiter};
use super::super::router::RequestAdmissionError;
use super::super::{control::GatewayControlDecision, error::GatewayError};
use super::super::{provider_transport, usage};

use crate::maintenance::spawn_audit_cleanup_worker;
use crate::maintenance::spawn_db_maintenance_worker;
use crate::maintenance::spawn_gemini_file_mapping_cleanup_worker;
use crate::maintenance::spawn_pending_cleanup_worker;
use crate::maintenance::spawn_pool_monitor_worker;
use crate::maintenance::spawn_provider_checkin_worker;
use crate::maintenance::spawn_proxy_node_stale_cleanup_worker;
use crate::maintenance::spawn_proxy_upgrade_rollout_worker;
use crate::maintenance::spawn_request_candidate_cleanup_worker;
use crate::maintenance::spawn_stats_aggregation_worker;
use crate::maintenance::spawn_stats_hourly_aggregation_worker;
use crate::maintenance::spawn_usage_cleanup_worker;
use crate::maintenance::spawn_wallet_daily_usage_aggregation_worker;

const SYSTEM_CONFIG_CACHE_TTL: Duration = Duration::from_secs(3);

impl AppState {
    fn spawn_scheduler_affinity_redis_write(
        &self,
        cache_key: &str,
        target: &SchedulerAffinityTarget,
        ttl: Duration,
    ) {
        let Some(runner) = self.redis_kv_runner() else {
            return;
        };
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };

        let cache_key = cache_key.to_string();
        let provider_id = target.provider_id.clone();
        let endpoint_id = target.endpoint_id.clone();
        let key_id = target.key_id.clone();
        let ttl_seconds = ttl.as_secs();
        let now_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;
        let expire_at = now_unix_secs.saturating_add(ttl_seconds);

        handle.spawn(async move {
            let payload = serde_json::json!({
                "provider_id": provider_id,
                "endpoint_id": endpoint_id,
                "key_id": key_id,
                "created_at": now_unix_secs,
                "expire_at": expire_at,
                "request_count": 0,
            });
            let Ok(serialized) = serde_json::to_string(&payload) else {
                return;
            };
            let _ = runner
                .setex(&cache_key, &serialized, Some(ttl_seconds))
                .await;
        });
    }

    pub(crate) fn replace_data_state(&mut self, data: Arc<GatewayDataState>) {
        self.clear_provider_transport_snapshot_cache();
        self.system_config_cache.clear();
        self.tunnel = crate::tunnel::EmbeddedTunnelState::with_data(Arc::clone(&data));
        self.data = data;
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
        let data = Arc::new(GatewayDataState::disabled());
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
            usage_runtime: Arc::new(usage::UsageRuntime::disabled()),
            video_tasks: Arc::new(VideoTaskService::new(
                VideoTaskTruthSourceMode::PythonSyncReport,
            )),
            video_task_poller: None,
            request_gate: None,
            distributed_request_gate: None,
            client,
            auth_context_cache: Arc::new(AuthContextCache::default()),
            auth_api_key_last_used_cache: Arc::new(AuthApiKeyLastUsedCache::default()),
            oauth_refresh: Arc::new(provider_transport::LocalOAuthRefreshCoordinator::new()),
            direct_plan_bypass_cache: Arc::new(DirectPlanBypassCache::default()),
            scheduler_affinity_cache: Arc::new(SchedulerAffinityCache::default()),
            dashboard_response_cache: Arc::new(DashboardResponseCache::default()),
            system_config_cache: Arc::new(SystemConfigCache::default()),
            fallback_metrics: Arc::new(fallback_metrics::GatewayFallbackMetrics::default()),
            frontdoor_cors: None,
            frontdoor_user_rpm: Arc::new(FrontdoorUserRpmLimiter::new(
                FrontdoorUserRpmConfig::default(),
            )),
            tunnel: crate::tunnel::EmbeddedTunnelState::with_data(data),
            provider_transport_snapshot_cache: Arc::new(StdMutex::new(HashMap::new())),
            provider_key_rpm_resets: Arc::new(StdMutex::new(HashMap::new())),
            local_execution_runtime_miss_diagnostics: Arc::new(StdMutex::new(HashMap::new())),
            admin_monitoring_error_stats_reset_at: Arc::new(StdMutex::new(None)),
            provider_delete_tasks: Arc::new(StdMutex::new(HashMap::new())),
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
        self.tunnel = crate::tunnel::EmbeddedTunnelState::with_data_and_identity(
            Arc::clone(&self.data),
            instance_id,
            relay_base_url,
            90,
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

    pub async fn run_postgres_migrations(&self) -> Result<bool, sqlx::migrate::MigrateError> {
        let Some(pool) = self.postgres_pool() else {
            return Ok(false);
        };
        aether_data::migrate::run_migrations(&pool).await?;
        Ok(true)
    }

    pub async fn run_postgres_backfills(&self) -> Result<bool, sqlx::migrate::MigrateError> {
        let Some(pool) = self.postgres_pool() else {
            return Ok(false);
        };
        aether_data::backfill::run_backfills(&pool).await?;
        Ok(true)
    }

    pub async fn pending_postgres_migrations(
        &self,
    ) -> Result<Option<Vec<aether_data::migrate::PendingMigrationInfo>>, sqlx::migrate::MigrateError>
    {
        let Some(pool) = self.postgres_pool() else {
            return Ok(None);
        };
        Ok(Some(aether_data::migrate::pending_migrations(&pool).await?))
    }

    pub async fn prepare_postgres_for_startup(
        &self,
    ) -> Result<Option<Vec<aether_data::migrate::PendingMigrationInfo>>, sqlx::migrate::MigrateError>
    {
        let Some(pool) = self.postgres_pool() else {
            return Ok(None);
        };
        Ok(Some(
            aether_data::migrate::prepare_database_for_startup(&pool).await?,
        ))
    }

    pub async fn pending_postgres_backfills(
        &self,
    ) -> Result<Option<Vec<aether_data::backfill::PendingBackfillInfo>>, sqlx::migrate::MigrateError>
    {
        let Some(pool) = self.postgres_pool() else {
            return Ok(None);
        };
        Ok(Some(aether_data::backfill::pending_backfills(&pool).await?))
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

    pub fn with_distributed_request_concurrency_gate(
        mut self,
        gate: DistributedConcurrencyGate,
    ) -> Self {
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

        let value = self
            .data
            .find_system_config_value(key)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        self.system_config_cache
            .insert(key.to_string(), value.clone(), SYSTEM_CONFIG_CACHE_TTL);
        Ok(value)
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
        self.system_config_cache.insert(
            key.to_string(),
            Some(value.clone()),
            SYSTEM_CONFIG_CACHE_TTL,
        );
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
        self.data
            .upsert_system_config_entry(key, value, description)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn delete_system_config_value(&self, key: &str) -> Result<bool, GatewayError> {
        let deleted = self
            .data
            .delete_system_config_value(key)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        self.system_config_cache
            .insert(key.to_string(), None, SYSTEM_CONFIG_CACHE_TTL);
        Ok(deleted)
    }

    pub(crate) async fn read_admin_system_stats(
        &self,
    ) -> Result<aether_data::repository::system::AdminSystemStats, GatewayError> {
        self.data
            .read_admin_system_stats()
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

    pub(crate) async fn apply_proxy_node_heartbeat(
        &self,
        mutation: &ProxyNodeHeartbeatMutation,
    ) -> Result<Option<StoredProxyNode>, GatewayError> {
        self.data
            .apply_proxy_node_heartbeat(mutation)
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
    ) -> Result<Option<DistributedConcurrencySnapshot>, DistributedConcurrencyError> {
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
        self.data.has_redis_backend()
    }

    pub(crate) fn redis_kv_runner(&self) -> Option<aether_data::redis::RedisKvRunner> {
        self.data.kv_runner()
    }

    pub(crate) fn postgres_pool(&self) -> Option<aether_data::postgres::PostgresPool> {
        self.data.postgres_pool()
    }

    pub(crate) fn remove_scheduler_affinity_cache_entry(&self, cache_key: &str) -> bool {
        self.scheduler_affinity_cache.remove(cache_key).is_some()
    }

    pub(crate) fn read_scheduler_affinity_target(
        &self,
        cache_key: &str,
        ttl: Duration,
    ) -> Option<SchedulerAffinityTarget> {
        self.scheduler_affinity_cache.get_fresh(cache_key, ttl)
    }

    pub(crate) fn remember_scheduler_affinity_target(
        &self,
        cache_key: &str,
        target: SchedulerAffinityTarget,
        ttl: Duration,
        max_entries: usize,
    ) {
        self.spawn_scheduler_affinity_redis_write(cache_key, &target, ttl);
        self.scheduler_affinity_cache
            .insert(cache_key.to_string(), target, ttl, max_entries);
    }

    pub(crate) fn list_scheduler_affinity_entries(
        &self,
        ttl: Duration,
    ) -> Vec<SchedulerAffinitySnapshotEntry> {
        self.scheduler_affinity_cache.fresh_entries(ttl)
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

    pub fn spawn_background_tasks(&self) -> Vec<JoinHandle<()>> {
        let mut tasks = Vec::new();
        if let Some(handle) = self.usage_runtime.spawn_worker(self.data.clone()) {
            tasks.push(handle);
        }
        if let Some(handle) =
            crate::wallet_runtime::spawn_provider_quota_reset_worker(self.data.clone())
        {
            tasks.push(handle);
        }
        if let Some(handle) = spawn_audit_cleanup_worker(self.data.clone()) {
            tasks.push(handle);
        }
        if let Some(handle) = spawn_db_maintenance_worker(self.data.clone()) {
            tasks.push(handle);
        }
        if let Some(handle) = spawn_wallet_daily_usage_aggregation_worker(self.data.clone()) {
            tasks.push(handle);
        }
        if let Some(handle) = spawn_stats_aggregation_worker(self.data.clone()) {
            tasks.push(handle);
        }
        if let Some(handle) = spawn_usage_cleanup_worker(self.data.clone()) {
            tasks.push(handle);
        }
        if let Some(handle) = spawn_pool_monitor_worker(self.data.clone()) {
            tasks.push(handle);
        }
        if let Some(handle) = spawn_stats_hourly_aggregation_worker(self.data.clone()) {
            tasks.push(handle);
        }
        if let Some(handle) = spawn_pending_cleanup_worker(self.data.clone()) {
            tasks.push(handle);
        }
        if let Some(handle) = spawn_proxy_node_stale_cleanup_worker(self.data.clone()) {
            tasks.push(handle);
        }
        if let Some(handle) = spawn_proxy_upgrade_rollout_worker(self.clone()) {
            tasks.push(handle);
        }
        if let Some(handle) = spawn_provider_checkin_worker(self.clone()) {
            tasks.push(handle);
        }
        if let Some(handle) = spawn_request_candidate_cleanup_worker(self.data.clone()) {
            tasks.push(handle);
        }
        if let Some(handle) = spawn_gemini_file_mapping_cleanup_worker(self.data.clone()) {
            tasks.push(handle);
        }
        if let Some(handle) = spawn_model_fetch_worker(self.clone()) {
            tasks.push(handle);
        }
        if let Some(handle) = spawn_video_task_poller(self.clone()) {
            tasks.push(handle);
        }
        tasks
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

    use serde_json::json;

    use super::AppState;
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
}
