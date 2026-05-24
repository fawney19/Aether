use super::AdminAppState;
use crate::GatewayError;

mod adaptive;
mod export;
mod import;
mod modules;
mod proxy_nodes;
mod templates;

const ADMIN_SYSTEM_DATA_EXPORT_VERSION: &str = "1.0";
const ADMIN_SYSTEM_DATA_IMPORT_MAX_SIZE_BYTES: usize = 500 * 1024 * 1024;

impl<'a> AdminAppState<'a> {
    pub(crate) async fn upsert_system_config_json_value(
        &self,
        key: &str,
        value: &serde_json::Value,
        description: Option<&str>,
    ) -> Result<serde_json::Value, GatewayError> {
        self.app
            .upsert_system_config_json_value(key, value, description)
            .await
    }

    pub(crate) async fn read_system_config_json_value(
        &self,
        key: &str,
    ) -> Result<Option<serde_json::Value>, GatewayError> {
        self.app.read_system_config_json_value(key).await
    }

    pub(crate) async fn list_system_config_entries(
        &self,
    ) -> Result<Vec<crate::data::state::StoredSystemConfigEntry>, GatewayError> {
        self.app.list_system_config_entries().await
    }

    pub(crate) async fn upsert_system_config_entry(
        &self,
        key: &str,
        value: &serde_json::Value,
        description: Option<&str>,
    ) -> Result<crate::data::state::StoredSystemConfigEntry, GatewayError> {
        self.app
            .upsert_system_config_entry(key, value, description)
            .await
    }

    pub(crate) async fn delete_system_config_value(&self, key: &str) -> Result<bool, GatewayError> {
        self.app.delete_system_config_value(key).await
    }

    pub(crate) async fn list_proxy_nodes(
        &self,
    ) -> Result<Vec<aether_data::repository::proxy_nodes::StoredProxyNode>, GatewayError> {
        self.app.list_proxy_nodes().await
    }

    pub(crate) async fn read_admin_system_stats(
        &self,
    ) -> Result<aether_data::repository::system::AdminSystemStats, GatewayError> {
        self.app.read_admin_system_stats().await
    }

    pub(crate) async fn purge_admin_system_data(
        &self,
        target: aether_data::repository::system::AdminSystemPurgeTarget,
    ) -> Result<aether_data::repository::system::AdminSystemPurgeSummary, GatewayError> {
        self.app.purge_admin_system_data(target).await
    }

    pub(crate) async fn export_admin_system_usage_aggregates(
        &self,
    ) -> Result<aether_data::repository::system::AdminSystemUsageAggregateSnapshot, GatewayError>
    {
        self.app.export_admin_system_usage_aggregates().await
    }

    pub(crate) async fn import_admin_system_usage_aggregates(
        &self,
        snapshot: &aether_data::repository::system::AdminSystemUsageAggregateSnapshot,
        user_id_map: &std::collections::BTreeMap<String, String>,
        api_key_id_map: &std::collections::BTreeMap<String, String>,
        mode: aether_data::repository::system::AdminSystemUsageAggregateImportMode,
    ) -> Result<aether_data::repository::system::AdminSystemUsageAggregateImportSummary, GatewayError>
    {
        self.app
            .import_admin_system_usage_aggregates(snapshot, user_id_map, api_key_id_map, mode)
            .await
    }

    pub(crate) async fn run_admin_system_cleanup_once(
        &self,
    ) -> Result<crate::maintenance::AdminSystemCleanupSummary, GatewayError> {
        self.app.run_admin_system_cleanup_once().await
    }

    pub(crate) async fn rebuild_admin_stats_once(
        &self,
    ) -> Result<crate::maintenance::AdminStatsRebuildSummary, GatewayError> {
        self.app.rebuild_admin_stats_once().await
    }

    pub(crate) async fn find_proxy_node(
        &self,
        node_id: &str,
    ) -> Result<Option<aether_data::repository::proxy_nodes::StoredProxyNode>, GatewayError> {
        self.app.find_proxy_node(node_id).await
    }

    pub(crate) async fn list_proxy_node_events(
        &self,
        node_id: &str,
        limit: usize,
    ) -> Result<Vec<aether_data::repository::proxy_nodes::StoredProxyNodeEvent>, GatewayError> {
        self.app.list_proxy_node_events(node_id, limit).await
    }

    pub(crate) async fn read_admin_email_template_payload(
        &self,
        template_type: &str,
    ) -> Result<Option<serde_json::Value>, GatewayError> {
        crate::handlers::shared::read_admin_email_template_payload(self.app, template_type).await
    }
}
