use std::time::Duration;

use aether_contracts::{ExecutionPlan, ExecutionResult, ProxySnapshot};
use aether_data_contracts::repository::candidates::{
    StoredRequestCandidate, UpsertRequestCandidateRecord,
};
use aether_data_contracts::repository::global_models::{
    AdminGlobalModelListQuery, AdminProviderModelListQuery, StoredAdminGlobalModelPage,
    StoredAdminProviderModel, UpsertAdminProviderModelRecord,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_data_contracts::repository::quota::StoredProviderQuotaSnapshot;
use aether_model_fetch::{
    aggregate_models_for_cache, model_fetch_interval_minutes, ModelFetchAssociationStore,
    ModelFetchTransportRuntime,
};
use aether_scheduler_core::SchedulerAffinityTarget;
use async_trait::async_trait;
use serde_json::Value;
use tracing::debug;

use super::{AppState, GatewayError};
use crate::model_fetch::ModelFetchRuntimeState;
use crate::provider_transport::{GatewayProviderTransportSnapshot, LocalResolvedOAuthRequestAuth};
use crate::request_candidate_runtime::{
    RequestCandidateRuntimeCapabilityReader, RequestCandidateRuntimeReader,
    RequestCandidateRuntimeWriter,
};
use crate::scheduler::state::SchedulerRuntimeState;
use crate::{execution_runtime, provider_transport};

#[async_trait]
impl provider_transport::TransportTunnelAffinityLookup for AppState {
    async fn lookup_tunnel_attachment_owner(
        &self,
        node_id: &str,
    ) -> Result<Option<provider_transport::TransportTunnelAttachmentOwner>, String> {
        self.tunnel
            .lookup_attachment_owner(self.data.as_ref(), node_id)
            .await
            .map(|owner| {
                owner.map(|owner| provider_transport::TransportTunnelAttachmentOwner {
                    gateway_instance_id: owner.gateway_instance_id,
                    relay_base_url: owner.relay_base_url,
                    observed_at_unix_secs: owner.observed_at_unix_secs,
                })
            })
    }
}

#[async_trait]
impl provider_transport::VideoTaskTransportSnapshotLookup for AppState {
    async fn read_video_task_provider_transport_snapshot(
        &self,
        provider_id: &str,
        endpoint_id: &str,
        key_id: &str,
    ) -> Result<Option<GatewayProviderTransportSnapshot>, String> {
        self.read_provider_transport_snapshot(provider_id, endpoint_id, key_id)
            .await
            .map_err(|err| match err {
                GatewayError::UpstreamUnavailable { message, .. }
                | GatewayError::ControlUnavailable { message, .. }
                | GatewayError::Client { message, .. }
                | GatewayError::Internal(message) => message,
            })
    }
}

#[async_trait]
impl ModelFetchTransportRuntime for AppState {
    async fn resolve_local_oauth_request_auth(
        &self,
        transport: &GatewayProviderTransportSnapshot,
    ) -> Result<Option<LocalResolvedOAuthRequestAuth>, String> {
        AppState::resolve_local_oauth_request_auth(self, transport)
            .await
            .map_err(|err| match err {
                GatewayError::UpstreamUnavailable { message, .. }
                | GatewayError::ControlUnavailable { message, .. }
                | GatewayError::Client { message, .. }
                | GatewayError::Internal(message) => message,
            })
    }

    async fn resolve_model_fetch_proxy(
        &self,
        transport: &GatewayProviderTransportSnapshot,
    ) -> Option<ProxySnapshot> {
        self.resolve_transport_proxy_snapshot_with_tunnel_affinity(transport)
            .await
    }

    async fn execute_model_fetch_execution_plan(
        &self,
        plan: &ExecutionPlan,
    ) -> Result<ExecutionResult, String> {
        execution_runtime::execute_execution_runtime_sync_plan(self, None, plan)
            .await
            .map_err(|err| match err {
                GatewayError::UpstreamUnavailable { message, .. }
                | GatewayError::ControlUnavailable { message, .. }
                | GatewayError::Client { message, .. }
                | GatewayError::Internal(message) => message,
            })
    }
}

#[async_trait]
impl ModelFetchRuntimeState for AppState {
    fn has_provider_catalog_data_reader(&self) -> bool {
        AppState::has_provider_catalog_data_reader(self)
    }

    fn has_provider_catalog_data_writer(&self) -> bool {
        AppState::has_provider_catalog_data_writer(self)
    }

    async fn list_provider_catalog_providers(
        &self,
        active_only: bool,
    ) -> Result<Vec<StoredProviderCatalogProvider>, GatewayError> {
        AppState::list_provider_catalog_providers(self, active_only).await
    }

    async fn list_provider_catalog_endpoints_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogEndpoint>, GatewayError> {
        AppState::list_provider_catalog_endpoints_by_provider_ids(self, provider_ids).await
    }

    async fn read_provider_transport_snapshot(
        &self,
        provider_id: &str,
        endpoint_id: &str,
        key_id: &str,
    ) -> Result<Option<GatewayProviderTransportSnapshot>, GatewayError> {
        AppState::read_provider_transport_snapshot(self, provider_id, endpoint_id, key_id).await
    }

    async fn execute_execution_runtime_sync_plan(
        &self,
        plan: &ExecutionPlan,
    ) -> Result<ExecutionResult, GatewayError> {
        execution_runtime::execute_execution_runtime_sync_plan(self, None, plan).await
    }

    async fn update_provider_catalog_key(
        &self,
        key: &StoredProviderCatalogKey,
    ) -> Result<(), GatewayError> {
        AppState::update_provider_catalog_key(self, key).await?;
        Ok(())
    }

    async fn write_upstream_models_cache(
        &self,
        provider_id: &str,
        key_id: &str,
        cached_models: &[Value],
    ) {
        let Ok(serialized) = serde_json::to_string(&aggregate_models_for_cache(cached_models))
        else {
            return;
        };
        let cache_key = format!("upstream_models:{provider_id}:{key_id}");
        if let Err(err) = self
            .runtime_state
            .kv_set(
                &cache_key,
                serialized,
                Some(std::time::Duration::from_secs(
                    model_fetch_interval_minutes().saturating_mul(60),
                )),
            )
            .await
        {
            debug!(
                provider_id = %provider_id,
                key_id = %key_id,
                error = %err,
                "gateway model fetch cache write failed"
            );
        }
    }
}

#[async_trait]
impl ModelFetchAssociationStore for AppState {
    type Error = String;

    fn has_global_model_reader(&self) -> bool {
        self.data.has_global_model_reader()
    }

    fn has_global_model_writer(&self) -> bool {
        self.data.has_global_model_writer()
    }

    fn model_fetch_internal_error(&self, message: String) -> Self::Error {
        message
    }

    async fn list_admin_provider_models(
        &self,
        query: &AdminProviderModelListQuery,
    ) -> Result<Vec<StoredAdminProviderModel>, Self::Error> {
        AppState::list_admin_provider_models(self, query)
            .await
            .map_err(|err| format!("{err:?}"))
    }

    async fn list_admin_global_models(
        &self,
        query: &AdminGlobalModelListQuery,
    ) -> Result<StoredAdminGlobalModelPage, Self::Error> {
        AppState::list_admin_global_models(self, query)
            .await
            .map_err(|err| format!("{err:?}"))
    }

    async fn create_admin_provider_model(
        &self,
        record: &UpsertAdminProviderModelRecord,
    ) -> Result<Option<StoredAdminProviderModel>, Self::Error> {
        AppState::create_admin_provider_model(self, record)
            .await
            .map_err(|err| format!("{err:?}"))
    }

    async fn list_provider_catalog_keys_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, Self::Error> {
        AppState::list_provider_catalog_keys_by_provider_ids(self, provider_ids)
            .await
            .map_err(|err| format!("{err:?}"))
    }
}

#[async_trait]
impl RequestCandidateRuntimeReader for AppState {
    async fn read_request_candidates_by_request_id(
        &self,
        request_id: &str,
    ) -> Result<Vec<StoredRequestCandidate>, GatewayError> {
        AppState::read_request_candidates_by_request_id(self, request_id).await
    }
}

#[async_trait]
impl RequestCandidateRuntimeCapabilityReader for AppState {
    async fn read_request_candidate_user_model_capability_settings(
        &self,
        user_id: &str,
    ) -> Result<Option<Value>, GatewayError> {
        AppState::read_user_model_capability_settings(self, user_id).await
    }

    async fn read_request_candidate_api_key_force_capabilities(
        &self,
        user_id: &str,
        api_key_id: &str,
    ) -> Result<Option<Value>, GatewayError> {
        AppState::read_auth_api_key_force_capabilities(self, user_id, api_key_id).await
    }
}

#[async_trait]
impl RequestCandidateRuntimeWriter for AppState {
    fn has_request_candidate_data_writer(&self) -> bool {
        AppState::has_request_candidate_data_writer(self)
    }

    async fn upsert_request_candidate(
        &self,
        candidate: UpsertRequestCandidateRecord,
    ) -> Result<Option<StoredRequestCandidate>, GatewayError> {
        AppState::upsert_request_candidate(self, candidate).await
    }
}

#[async_trait]
impl SchedulerRuntimeState for AppState {
    async fn read_provider_quota_snapshot(
        &self,
        provider_id: &str,
    ) -> Result<Option<StoredProviderQuotaSnapshot>, GatewayError> {
        AppState::read_provider_quota_snapshot(self, provider_id).await
    }

    async fn read_provider_catalog_providers_by_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogProvider>, GatewayError> {
        AppState::read_provider_catalog_providers_by_ids(self, provider_ids).await
    }

    async fn read_provider_catalog_keys_by_ids(
        &self,
        key_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, GatewayError> {
        AppState::read_provider_catalog_keys_by_ids(self, key_ids).await
    }

    async fn read_recent_request_candidates(
        &self,
        limit: usize,
    ) -> Result<Vec<StoredRequestCandidate>, GatewayError> {
        AppState::read_recent_request_candidates(self, limit).await
    }

    fn provider_key_rpm_reset_at(&self, key_id: &str, now_unix_secs: u64) -> Option<u64> {
        AppState::provider_key_rpm_reset_at(self, key_id, now_unix_secs)
    }

    fn read_cached_scheduler_affinity_target(
        &self,
        cache_key: &str,
        ttl: Duration,
    ) -> Option<SchedulerAffinityTarget> {
        AppState::read_scheduler_affinity_target(self, cache_key, ttl)
    }

    fn scheduler_affinity_epoch(&self) -> u64 {
        AppState::scheduler_affinity_epoch(self)
    }

    fn remember_scheduler_affinity_target(
        &self,
        cache_key: &str,
        target: SchedulerAffinityTarget,
        ttl: Duration,
        max_entries: usize,
    ) {
        AppState::remember_scheduler_affinity_target(self, cache_key, target, ttl, max_entries);
    }

    fn remember_scheduler_affinity_target_for_epoch(
        &self,
        cache_key: &str,
        target: SchedulerAffinityTarget,
        ttl: Duration,
        max_entries: usize,
        expected_epoch: Option<u64>,
    ) -> bool {
        AppState::remember_scheduler_affinity_target_for_epoch(
            self,
            cache_key,
            target,
            ttl,
            max_entries,
            expected_epoch,
        )
    }

    async fn read_scheduler_ordering_config(
        &self,
    ) -> Result<crate::scheduler::config::SchedulerOrderingConfig, GatewayError> {
        crate::scheduler::config::read_scheduler_ordering_config(self).await
    }
}
