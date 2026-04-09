use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use aether_admin::system::{
    build_admin_proxy_node_event_payload, build_admin_proxy_node_events_payload_response,
    build_admin_proxy_node_payload, build_admin_proxy_nodes_data_unavailable_response,
    build_admin_proxy_nodes_invalid_status_response, build_admin_proxy_nodes_list_payload_response,
    build_admin_proxy_nodes_not_found_response,
};
use axum::{body::Body, response::Response};

impl<'a> AdminAppState<'a> {
    pub(crate) async fn build_admin_proxy_nodes_list_response(
        &self,
        skip: usize,
        limit: usize,
        status: Option<String>,
    ) -> Result<Response<Body>, GatewayError> {
        if !self.has_proxy_node_reader() {
            return Ok(build_admin_proxy_nodes_data_unavailable_response());
        }
        if let Some(status) = status.as_deref() {
            if !matches!(status, "offline" | "online") {
                return Ok(build_admin_proxy_nodes_invalid_status_response());
            }
        }

        let mut nodes = self.list_proxy_nodes().await?;
        nodes.sort_by(|left, right| left.name.cmp(&right.name));
        let filtered = nodes
            .into_iter()
            .filter(|node| {
                status
                    .as_deref()
                    .map(|value| node.status.eq_ignore_ascii_case(value))
                    .unwrap_or(true)
            })
            .collect::<Vec<_>>();
        let total = filtered.len();
        let items = filtered
            .into_iter()
            .skip(skip)
            .take(limit)
            .map(|node| build_admin_proxy_node_payload(&node))
            .collect::<Vec<_>>();
        Ok(build_admin_proxy_nodes_list_payload_response(
            items, total, skip, limit,
        ))
    }

    pub(crate) async fn build_admin_proxy_node_events_response(
        &self,
        node_id: &str,
        limit: usize,
    ) -> Result<Response<Body>, GatewayError> {
        if !self.has_proxy_node_reader() {
            return Ok(build_admin_proxy_nodes_data_unavailable_response());
        }
        if self.find_proxy_node(node_id).await?.is_none() {
            return Ok(build_admin_proxy_nodes_not_found_response());
        }
        let items = self
            .list_proxy_node_events(node_id, limit)
            .await?
            .into_iter()
            .map(|event| build_admin_proxy_node_event_payload(&event))
            .collect::<Vec<_>>();
        Ok(build_admin_proxy_node_events_payload_response(items))
    }
}
