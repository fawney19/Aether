use crate::handlers::admin::request::AdminAppState;
use crate::handlers::shared::unix_secs_to_rfc3339;
use crate::maintenance::{inspect_proxy_upgrade_rollout, ProxyUpgradeRolloutStatus};
use crate::GatewayError;
use aether_admin::system::{
    build_admin_proxy_node_event_payload, build_admin_proxy_node_events_payload_response,
    build_admin_proxy_node_payload, build_admin_proxy_nodes_data_unavailable_response,
    build_admin_proxy_nodes_invalid_status_response, build_admin_proxy_nodes_list_payload_response,
    build_admin_proxy_nodes_not_found_response,
};
use axum::{body::Body, response::Response};

impl<'a> AdminAppState<'a> {
    pub(crate) async fn register_proxy_node(
        &self,
        mutation: &aether_data::repository::proxy_nodes::ProxyNodeRegistrationMutation,
    ) -> Result<Option<aether_data::repository::proxy_nodes::StoredProxyNode>, GatewayError> {
        self.app.register_proxy_node(mutation).await
    }

    pub(crate) async fn apply_proxy_node_heartbeat(
        &self,
        mutation: &aether_data::repository::proxy_nodes::ProxyNodeHeartbeatMutation,
    ) -> Result<Option<aether_data::repository::proxy_nodes::StoredProxyNode>, GatewayError> {
        self.app.apply_proxy_node_heartbeat(mutation).await
    }

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
        let rollout = inspect_proxy_upgrade_rollout(self.app().data.as_ref())
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?
            .map(build_admin_proxy_upgrade_rollout_payload);
        Ok(build_admin_proxy_nodes_list_payload_response(
            items, total, skip, limit, rollout,
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

    pub(crate) async fn unregister_proxy_node(
        &self,
        node_id: &str,
    ) -> Result<Option<aether_data::repository::proxy_nodes::StoredProxyNode>, GatewayError> {
        self.app.unregister_proxy_node(node_id).await
    }

    pub(crate) async fn update_proxy_node_remote_config(
        &self,
        mutation: &aether_data::repository::proxy_nodes::ProxyNodeRemoteConfigMutation,
    ) -> Result<Option<aether_data::repository::proxy_nodes::StoredProxyNode>, GatewayError> {
        self.app.update_proxy_node_remote_config(mutation).await
    }
}

fn build_admin_proxy_upgrade_rollout_payload(
    rollout: ProxyUpgradeRolloutStatus,
) -> serde_json::Value {
    serde_json::json!({
        "version": rollout.version,
        "batch_size": rollout.batch_size,
        "cooldown_secs": rollout.cooldown_secs,
        "started_at": unix_secs_to_rfc3339(rollout.started_at_unix_secs),
        "last_dispatched_at": rollout
            .last_dispatched_at_unix_secs
            .and_then(unix_secs_to_rfc3339),
        "updated_at": unix_secs_to_rfc3339(rollout.updated_at_unix_secs),
        "probe": rollout.probe.map(|probe| serde_json::json!({
            "url": probe.url,
            "timeout_secs": probe.timeout_secs,
        })),
        "blocked": rollout.blocked,
        "online_eligible_total": rollout.online_eligible_total,
        "completed_node_ids": rollout.completed_node_ids,
        "pending_node_ids": rollout.pending_node_ids,
        "conflict_node_ids": rollout.conflict_node_ids,
        "skipped_node_ids": rollout.skipped_node_ids,
        "tracked_nodes": rollout.tracked_nodes.into_iter().map(|tracked| serde_json::json!({
            "node_id": tracked.node_id,
            "state": tracked.state,
            "dispatched_at": unix_secs_to_rfc3339(tracked.dispatched_at_unix_secs),
            "version_confirmed_at": tracked
                .version_confirmed_at_unix_secs
                .and_then(unix_secs_to_rfc3339),
            "traffic_confirmed_at": tracked
                .traffic_confirmed_at_unix_secs
                .and_then(unix_secs_to_rfc3339),
            "cooldown_remaining_secs": tracked.cooldown_remaining_secs,
        })).collect::<Vec<_>>(),
    })
}
