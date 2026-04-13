use super::AdminAppState;
use crate::GatewayError;

mod adaptive;
mod export;
mod import;
mod modules;
mod proxy_nodes;
mod templates;

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
