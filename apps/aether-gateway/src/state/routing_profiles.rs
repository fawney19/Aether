use aether_data_contracts::repository::routing_profiles::{
    CreateRoutingGroupBindingRecord, CreateRoutingGroupRecord, CreateRoutingGroupVersionRecord,
    RoutingGroupBindingQuery, RoutingGroupLookupKey, RoutingGroupReadRepository,
    StoredRoutingGroup, StoredRoutingGroupBinding, StoredRoutingGroupVersion,
    UpdateRoutingGroupBindingRecord, UpdateRoutingGroupRecord,
};
use aether_routing_core::RoutingGroupConfig;
use std::sync::Arc;
use tracing::warn;

use super::{AppState, GatewayError};

const BOOTSTRAP_SYSTEM_DEFAULT_ROUTING_GROUP_NAME: &str = "system-default";

impl AppState {
    /// Make sure an enabled system-default routing group exists.
    ///
    /// When none exists, one is created from the routing defaults.
    /// Returns the created group, or `None` when nothing had to be created (no
    /// routing storage, no writer, or a system default already exists).
    pub async fn ensure_system_default_routing_group(
        &self,
    ) -> Result<Option<StoredRoutingGroup>, std::io::Error> {
        self.ensure_system_default_routing_group_inner()
            .await
            .map_err(|err| std::io::Error::other(format!("{err:?}")))
    }

    pub(crate) async fn ensure_system_default_routing_group_inner(
        &self,
    ) -> Result<Option<StoredRoutingGroup>, GatewayError> {
        if !self.has_routing_group_data_reader() {
            return Ok(None);
        }
        if self
            .find_routing_group(RoutingGroupLookupKey::SystemDefault)
            .await?
            .is_some()
        {
            return Ok(None);
        }
        if !self.has_routing_group_data_writer() {
            warn!(
                event_name = "routing_system_default_bootstrap_skipped",
                log_type = "event",
                "no system default routing group exists and routing storage is read-only; scheduler uses routing defaults"
            );
            return Ok(None);
        }

        let config = RoutingGroupConfig::default();
        let config_json = serde_json::to_value(config)
            .map_err(|err| GatewayError::Internal(format!("serialize routing config: {err}")))?;

        let name = if self
            .find_routing_group(RoutingGroupLookupKey::Name(
                BOOTSTRAP_SYSTEM_DEFAULT_ROUTING_GROUP_NAME,
            ))
            .await?
            .is_some()
        {
            format!(
                "{BOOTSTRAP_SYSTEM_DEFAULT_ROUTING_GROUP_NAME}-{}",
                &uuid::Uuid::new_v4().simple().to_string()[..8]
            )
        } else {
            BOOTSTRAP_SYSTEM_DEFAULT_ROUTING_GROUP_NAME.to_string()
        };
        let now = crate::clock::current_unix_secs() as i64;
        self.create_routing_group(CreateRoutingGroupRecord {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            description: Some("系统默认调度策略".to_string()),
            enabled: true,
            is_system_default: true,
            sort_order: 0,
            config_json,
            version: 1,
            created_at: now,
            updated_at: now,
            published_at: Some(now),
        })
        .await
    }

    pub(crate) fn has_routing_group_data_reader(&self) -> bool {
        self.data.has_routing_group_reader()
    }

    pub(crate) fn has_routing_group_data_writer(&self) -> bool {
        self.data.has_routing_group_writer()
    }

    pub(crate) fn routing_group_read_repository(
        &self,
    ) -> Option<Arc<dyn RoutingGroupReadRepository>> {
        self.data.routing_group_read_repository()
    }

    pub(crate) async fn list_routing_groups(
        &self,
    ) -> Result<Vec<StoredRoutingGroup>, GatewayError> {
        self.data
            .list_routing_groups()
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn find_routing_group(
        &self,
        lookup: RoutingGroupLookupKey<'_>,
    ) -> Result<Option<StoredRoutingGroup>, GatewayError> {
        self.data
            .find_routing_group(lookup)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_routing_group_bindings(
        &self,
        query: &RoutingGroupBindingQuery,
    ) -> Result<Vec<StoredRoutingGroupBinding>, GatewayError> {
        self.data
            .list_routing_group_bindings(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_routing_group_versions(
        &self,
        group_id: &str,
    ) -> Result<Vec<StoredRoutingGroupVersion>, GatewayError> {
        self.data
            .list_routing_group_versions(group_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn create_routing_group(
        &self,
        record: CreateRoutingGroupRecord,
    ) -> Result<Option<StoredRoutingGroup>, GatewayError> {
        let created = self
            .data
            .create_routing_group(record)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if created.is_some() {
            self.invalidate_provider_routing_caches();
        }
        Ok(created)
    }

    pub(crate) async fn update_routing_group(
        &self,
        id: &str,
        patch: UpdateRoutingGroupRecord,
    ) -> Result<Option<StoredRoutingGroup>, GatewayError> {
        let updated = self
            .data
            .update_routing_group(id, patch)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if updated.is_some() {
            self.invalidate_provider_routing_caches();
        }
        Ok(updated)
    }

    pub(crate) async fn delete_routing_group(&self, id: &str) -> Result<bool, GatewayError> {
        let deleted = self
            .data
            .delete_routing_group(id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if deleted {
            self.invalidate_provider_routing_caches();
        }
        Ok(deleted)
    }

    pub(crate) async fn create_routing_group_binding(
        &self,
        record: CreateRoutingGroupBindingRecord,
    ) -> Result<Option<StoredRoutingGroupBinding>, GatewayError> {
        let created = self
            .data
            .create_routing_group_binding(record)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if created.is_some() {
            self.invalidate_provider_routing_caches();
        }
        Ok(created)
    }

    pub(crate) async fn update_routing_group_binding(
        &self,
        id: &str,
        patch: UpdateRoutingGroupBindingRecord,
    ) -> Result<Option<StoredRoutingGroupBinding>, GatewayError> {
        let updated = self
            .data
            .update_routing_group_binding(id, patch)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if updated.is_some() {
            self.invalidate_provider_routing_caches();
        }
        Ok(updated)
    }

    pub(crate) async fn delete_routing_group_binding(
        &self,
        id: &str,
    ) -> Result<bool, GatewayError> {
        let deleted = self
            .data
            .delete_routing_group_binding(id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if deleted {
            self.invalidate_provider_routing_caches();
        }
        Ok(deleted)
    }

    pub(crate) async fn create_routing_group_version(
        &self,
        record: CreateRoutingGroupVersionRecord,
    ) -> Result<Option<StoredRoutingGroupVersion>, GatewayError> {
        self.data
            .create_routing_group_version(record)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }
}
