use aether_data::repository::proxy_nodes::{
    proxy_node_accepts_new_tunnels, proxy_reported_version, remote_config_upgrade_target,
    ProxyNodeRemoteConfigMutation, StoredProxyNode,
};
use aether_data_contracts::DataLayerError;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::data::GatewayDataState;

use super::now_unix_secs;

const PROXY_UPGRADE_ROLLOUT_CONFIG_KEY: &str = "proxy_node_upgrade_rollout";
const PROXY_UPGRADE_ROLLOUT_DESCRIPTION: &str = "gateway-managed proxy upgrade rollout state";
pub(super) const DEFAULT_PROXY_UPGRADE_ROLLOUT_COOLDOWN_SECS: u64 = 60;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ProxyUpgradeRolloutPlan {
    version: String,
    batch_size: usize,
    cooldown_secs: u64,
    #[serde(default)]
    probe: Option<ProxyUpgradeRolloutProbeConfig>,
    started_at_unix_secs: u64,
    last_dispatched_at_unix_secs: Option<u64>,
    updated_at_unix_secs: u64,
    #[serde(default)]
    skipped_node_ids: Vec<String>,
    #[serde(default)]
    tracked_nodes: Vec<ProxyUpgradeRolloutTrackedNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ProxyUpgradeRolloutProbeConfig {
    pub(crate) url: String,
    pub(crate) timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ProxyUpgradeRolloutTrackedNode {
    node_id: String,
    dispatched_at_unix_secs: u64,
    version_confirmed_at_unix_secs: Option<u64>,
    #[serde(default)]
    traffic_confirmed_at_unix_secs: Option<u64>,
    confirm_failed_requests: Option<i64>,
    confirm_dns_failures: Option<i64>,
    confirm_stream_errors: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProxyUpgradeRolloutPendingProbe {
    pub(crate) node_id: String,
    pub(crate) url: String,
    pub(crate) timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ProxyUpgradeRolloutStatus {
    pub(crate) version: String,
    pub(crate) batch_size: usize,
    pub(crate) cooldown_secs: u64,
    pub(crate) started_at_unix_secs: u64,
    pub(crate) last_dispatched_at_unix_secs: Option<u64>,
    pub(crate) updated_at_unix_secs: u64,
    #[serde(default)]
    pub(crate) probe: Option<ProxyUpgradeRolloutProbeConfig>,
    pub(crate) blocked: bool,
    pub(crate) online_eligible_total: usize,
    #[serde(default)]
    pub(crate) completed_node_ids: Vec<String>,
    #[serde(default)]
    pub(crate) pending_node_ids: Vec<String>,
    #[serde(default)]
    pub(crate) conflict_node_ids: Vec<String>,
    #[serde(default)]
    pub(crate) skipped_node_ids: Vec<String>,
    #[serde(default)]
    pub(crate) tracked_nodes: Vec<ProxyUpgradeRolloutTrackedNodeStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ProxyUpgradeRolloutTrackedNodeStatus {
    pub(crate) node_id: String,
    pub(crate) state: ProxyUpgradeRolloutTrackedNodeState,
    pub(crate) dispatched_at_unix_secs: u64,
    pub(crate) version_confirmed_at_unix_secs: Option<u64>,
    pub(crate) traffic_confirmed_at_unix_secs: Option<u64>,
    pub(crate) cooldown_remaining_secs: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProxyUpgradeRolloutTrackedNodeState {
    AwaitingVersion,
    AwaitingTraffic,
    CoolingDown,
    Unhealthy,
    ReadyToFinalize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProxyUpgradeRolloutSummary {
    pub version: String,
    pub batch_size: usize,
    pub cooldown_secs: u64,
    pub updated: usize,
    pub skipped: usize,
    pub blocked: bool,
    pub node_ids: Vec<String>,
    pub pending_node_ids: Vec<String>,
    pub rollout_active: bool,
    pub completed: usize,
    pub remaining: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProxyUpgradeRolloutCancelSummary {
    pub version: String,
    pub pending_node_ids: Vec<String>,
    pub conflict_node_ids: Vec<String>,
    pub completed: usize,
    pub remaining: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProxyUpgradeRolloutConflictClearSummary {
    pub version: String,
    pub cleared_node_ids: Vec<String>,
    pub updated: usize,
    pub blocked: bool,
    pub pending_node_ids: Vec<String>,
    pub rollout_active: bool,
    pub completed: usize,
    pub remaining: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProxyUpgradeRolloutSkippedRestoreSummary {
    pub version: String,
    pub restored_node_ids: Vec<String>,
    pub skipped_node_ids: Vec<String>,
    pub updated: usize,
    pub blocked: bool,
    pub pending_node_ids: Vec<String>,
    pub rollout_active: bool,
    pub completed: usize,
    pub remaining: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProxyUpgradeRolloutNodeActionSummary {
    pub version: String,
    pub node_id: String,
    pub skipped_node_ids: Vec<String>,
    pub updated: usize,
    pub blocked: bool,
    pub pending_node_ids: Vec<String>,
    pub rollout_active: bool,
    pub completed: usize,
    pub remaining: usize,
}

#[derive(Debug, Default)]
struct RolloutSnapshot {
    online_eligible_total: usize,
    completed: Vec<String>,
    pending: Vec<String>,
    pending_conflicts: Vec<String>,
    skipped: Vec<String>,
    available: Vec<StoredProxyNode>,
    ready_to_finalize: Vec<StoredProxyNode>,
    remaining_total: usize,
    tracked_nodes: Vec<ProxyUpgradeRolloutTrackedNode>,
}

pub(super) async fn advance_proxy_upgrade_rollout_once(
    data: &GatewayDataState,
) -> Result<ProxyUpgradeRolloutSummary, DataLayerError> {
    ensure_proxy_upgrade_rollout_storage(data)?;
    advance_proxy_upgrade_rollout(data).await
}

pub(crate) async fn start_proxy_upgrade_rollout(
    data: &GatewayDataState,
    version: String,
    batch_size: usize,
    cooldown_secs: u64,
    probe: Option<ProxyUpgradeRolloutProbeConfig>,
) -> Result<ProxyUpgradeRolloutSummary, DataLayerError> {
    ensure_proxy_upgrade_rollout_storage(data)?;
    let now = now_unix_secs();
    let normalized_version = version.trim().to_string();
    let existing = load_proxy_upgrade_rollout_plan(data).await?;
    if let Some(active_plan) = existing
        .as_ref()
        .filter(|plan| !same_rollout_version(plan.version.as_str(), normalized_version.as_str()))
    {
        let snapshot = build_rollout_snapshot(active_plan, data.list_proxy_nodes().await?, now);
        return Ok(ProxyUpgradeRolloutSummary {
            version: active_plan.version.clone(),
            batch_size: active_plan.batch_size,
            cooldown_secs: active_plan.cooldown_secs,
            updated: 0,
            skipped: snapshot.online_eligible_total,
            blocked: true,
            node_ids: Vec::new(),
            pending_node_ids: if snapshot.pending_conflicts.is_empty() {
                snapshot.pending
            } else {
                snapshot.pending_conflicts
            },
            rollout_active: true,
            completed: snapshot.completed.len(),
            remaining: snapshot.remaining_total,
        });
    }
    let preserve_existing = existing.as_ref().is_some_and(|plan| {
        same_rollout_version(plan.version.as_str(), normalized_version.as_str())
    });
    let resolved_probe = resolve_rollout_probe_config(
        existing
            .as_ref()
            .filter(|_| preserve_existing)
            .and_then(|plan| plan.probe.as_ref()),
        probe,
    );
    let plan = ProxyUpgradeRolloutPlan {
        version: normalized_version,
        batch_size: batch_size.max(1),
        cooldown_secs,
        probe: resolved_probe,
        started_at_unix_secs: existing
            .as_ref()
            .filter(|_| preserve_existing)
            .map(|plan| plan.started_at_unix_secs)
            .unwrap_or(now),
        last_dispatched_at_unix_secs: existing
            .as_ref()
            .filter(|_| preserve_existing)
            .and_then(|plan| plan.last_dispatched_at_unix_secs),
        updated_at_unix_secs: now,
        skipped_node_ids: existing
            .as_ref()
            .filter(|_| preserve_existing)
            .map(|plan| plan.skipped_node_ids.clone())
            .unwrap_or_default(),
        tracked_nodes: existing
            .as_ref()
            .filter(|_| preserve_existing)
            .map(|plan| plan.tracked_nodes.clone())
            .unwrap_or_default(),
    };
    let snapshot = build_rollout_snapshot(&plan, data.list_proxy_nodes().await?, now);
    if !snapshot.pending_conflicts.is_empty() {
        return Ok(ProxyUpgradeRolloutSummary {
            version: plan.version.clone(),
            batch_size: plan.batch_size,
            cooldown_secs: plan.cooldown_secs,
            updated: 0,
            skipped: snapshot.online_eligible_total,
            blocked: true,
            node_ids: Vec::new(),
            pending_node_ids: snapshot.pending_conflicts,
            rollout_active: existing.is_some(),
            completed: snapshot.completed.len(),
            remaining: snapshot.remaining_total,
        });
    }
    save_proxy_upgrade_rollout_plan(data, &plan).await?;
    advance_proxy_upgrade_rollout(data).await
}

pub(crate) async fn collect_proxy_upgrade_rollout_probes(
    data: &GatewayDataState,
) -> Result<Vec<ProxyUpgradeRolloutPendingProbe>, DataLayerError> {
    if !data.has_system_config_store() {
        return Ok(Vec::new());
    }

    let Some(plan) = load_proxy_upgrade_rollout_plan(data).await? else {
        return Ok(Vec::new());
    };
    let Some(probe) = plan.probe.as_ref() else {
        return Ok(Vec::new());
    };

    let now = now_unix_secs();
    let nodes = data.list_proxy_nodes().await?;
    let snapshot = build_rollout_snapshot(&plan, nodes.clone(), now);
    let nodes_by_id = nodes
        .into_iter()
        .map(|node| (node.id.clone(), node))
        .collect::<std::collections::BTreeMap<_, _>>();

    Ok(snapshot
        .tracked_nodes
        .into_iter()
        .filter(|tracked| {
            tracked.version_confirmed_at_unix_secs.is_some()
                && tracked.traffic_confirmed_at_unix_secs.is_none()
        })
        .filter_map(|tracked| {
            let node = nodes_by_id.get(&tracked.node_id)?;
            if !node.status.eq_ignore_ascii_case("online") || !node.tunnel_connected {
                return None;
            }
            Some(ProxyUpgradeRolloutPendingProbe {
                node_id: tracked.node_id,
                url: probe.url.clone(),
                timeout_secs: probe.timeout_secs,
            })
        })
        .collect())
}

pub(crate) async fn inspect_proxy_upgrade_rollout(
    data: &GatewayDataState,
) -> Result<Option<ProxyUpgradeRolloutStatus>, DataLayerError> {
    if !data.has_system_config_store() {
        return Ok(None);
    }

    let Some(plan) = load_proxy_upgrade_rollout_plan(data).await? else {
        return Ok(None);
    };
    let now = now_unix_secs();
    let nodes = data.list_proxy_nodes().await?;
    let snapshot = build_rollout_snapshot(&plan, nodes.clone(), now);
    let nodes_by_id = nodes
        .into_iter()
        .map(|node| (node.id.clone(), node))
        .collect::<std::collections::BTreeMap<_, _>>();

    let mut tracked_nodes = snapshot
        .tracked_nodes
        .iter()
        .map(|tracked| {
            build_rollout_tracked_node_status(
                tracked,
                nodes_by_id.get(tracked.node_id.as_str()),
                plan.cooldown_secs,
                now,
            )
        })
        .collect::<Vec<_>>();

    for node in &snapshot.ready_to_finalize {
        let Some(tracked) = plan
            .tracked_nodes
            .iter()
            .find(|tracked| tracked.node_id == node.id)
        else {
            continue;
        };
        tracked_nodes.push(ProxyUpgradeRolloutTrackedNodeStatus {
            node_id: tracked.node_id.clone(),
            state: ProxyUpgradeRolloutTrackedNodeState::ReadyToFinalize,
            dispatched_at_unix_secs: tracked.dispatched_at_unix_secs,
            version_confirmed_at_unix_secs: tracked.version_confirmed_at_unix_secs,
            traffic_confirmed_at_unix_secs: tracked.traffic_confirmed_at_unix_secs,
            cooldown_remaining_secs: Some(0),
        });
    }

    tracked_nodes.sort_by(|left, right| left.node_id.cmp(&right.node_id));

    Ok(Some(ProxyUpgradeRolloutStatus {
        version: plan.version,
        batch_size: plan.batch_size,
        cooldown_secs: plan.cooldown_secs,
        started_at_unix_secs: plan.started_at_unix_secs,
        last_dispatched_at_unix_secs: plan.last_dispatched_at_unix_secs,
        updated_at_unix_secs: plan.updated_at_unix_secs,
        probe: plan.probe,
        blocked: !snapshot.pending.is_empty() || !snapshot.pending_conflicts.is_empty(),
        online_eligible_total: snapshot.online_eligible_total,
        completed_node_ids: snapshot.completed,
        pending_node_ids: snapshot.pending,
        conflict_node_ids: snapshot.pending_conflicts,
        skipped_node_ids: snapshot.skipped,
        tracked_nodes,
    }))
}

pub(crate) async fn cancel_proxy_upgrade_rollout(
    data: &GatewayDataState,
) -> Result<Option<ProxyUpgradeRolloutCancelSummary>, DataLayerError> {
    ensure_proxy_upgrade_rollout_storage(data)?;

    let Some(plan) = load_proxy_upgrade_rollout_plan(data).await? else {
        return Ok(None);
    };
    let now = now_unix_secs();
    let snapshot = build_rollout_snapshot(&plan, data.list_proxy_nodes().await?, now);

    data.delete_system_config_value(PROXY_UPGRADE_ROLLOUT_CONFIG_KEY)
        .await?;

    Ok(Some(ProxyUpgradeRolloutCancelSummary {
        version: plan.version,
        pending_node_ids: snapshot.pending,
        conflict_node_ids: snapshot.pending_conflicts,
        completed: snapshot.completed.len(),
        remaining: snapshot.remaining_total,
    }))
}

pub(crate) async fn clear_proxy_upgrade_rollout_conflicts(
    data: &GatewayDataState,
) -> Result<Option<ProxyUpgradeRolloutConflictClearSummary>, DataLayerError> {
    ensure_proxy_upgrade_rollout_storage(data)?;

    let Some(plan) = load_proxy_upgrade_rollout_plan(data).await? else {
        return Ok(None);
    };
    let now = now_unix_secs();
    let snapshot = build_rollout_snapshot(&plan, data.list_proxy_nodes().await?, now);
    if snapshot.pending_conflicts.is_empty() {
        return Ok(Some(ProxyUpgradeRolloutConflictClearSummary {
            version: plan.version,
            cleared_node_ids: Vec::new(),
            updated: 0,
            blocked: !snapshot.pending.is_empty(),
            pending_node_ids: snapshot.pending,
            rollout_active: true,
            completed: snapshot.completed.len(),
            remaining: snapshot.remaining_total,
        }));
    }

    let mut cleared_node_ids = Vec::new();
    for node_id in snapshot.pending_conflicts {
        let Some(updated) = data
            .update_proxy_node_remote_config(&ProxyNodeRemoteConfigMutation {
                node_id: node_id.clone(),
                node_name: None,
                allowed_ports: None,
                log_level: None,
                heartbeat_interval: None,
                scheduling_state: None,
                upgrade_to: Some(None),
            })
            .await?
        else {
            continue;
        };
        cleared_node_ids.push(updated.id);
    }

    let rollout = advance_proxy_upgrade_rollout(data).await?;
    Ok(Some(ProxyUpgradeRolloutConflictClearSummary {
        version: if rollout.version.is_empty() {
            plan.version
        } else {
            rollout.version
        },
        cleared_node_ids,
        updated: rollout.updated,
        blocked: rollout.blocked,
        pending_node_ids: rollout.pending_node_ids,
        rollout_active: rollout.rollout_active,
        completed: rollout.completed,
        remaining: rollout.remaining,
    }))
}

pub(crate) async fn skip_proxy_upgrade_rollout_node(
    data: &GatewayDataState,
    node_id: &str,
) -> Result<Option<ProxyUpgradeRolloutNodeActionSummary>, DataLayerError> {
    ensure_proxy_upgrade_rollout_storage(data)?;

    let Some(mut plan) = load_proxy_upgrade_rollout_plan(data).await? else {
        return Ok(None);
    };
    let Some(node) = data.find_proxy_node(node_id).await? else {
        return Ok(None);
    };
    let now = now_unix_secs();

    if !plan.skipped_node_ids.iter().any(|id| id == node_id) {
        plan.skipped_node_ids.push(node_id.to_string());
        plan.skipped_node_ids.sort();
        plan.skipped_node_ids.dedup();
    }
    plan.tracked_nodes
        .retain(|tracked| tracked.node_id != node_id);
    plan.updated_at_unix_secs = now;
    save_proxy_upgrade_rollout_plan(data, &plan).await?;

    if remote_config_upgrade_target(node.remote_config.as_ref())
        == Some(normalize_rollout_version(plan.version.as_str()))
    {
        let _ = data
            .update_proxy_node_remote_config(&ProxyNodeRemoteConfigMutation {
                node_id: node_id.to_string(),
                node_name: None,
                allowed_ports: None,
                log_level: None,
                heartbeat_interval: None,
                scheduling_state: None,
                upgrade_to: Some(None),
            })
            .await?;
    }

    let rollout = advance_proxy_upgrade_rollout(data).await?;
    Ok(Some(ProxyUpgradeRolloutNodeActionSummary {
        version: if rollout.version.is_empty() {
            plan.version
        } else {
            rollout.version
        },
        node_id: node_id.to_string(),
        skipped_node_ids: plan.skipped_node_ids,
        updated: rollout.updated,
        blocked: rollout.blocked,
        pending_node_ids: rollout.pending_node_ids,
        rollout_active: rollout.rollout_active,
        completed: rollout.completed,
        remaining: rollout.remaining,
    }))
}

pub(crate) async fn restore_proxy_upgrade_rollout_skipped_nodes(
    data: &GatewayDataState,
) -> Result<Option<ProxyUpgradeRolloutSkippedRestoreSummary>, DataLayerError> {
    ensure_proxy_upgrade_rollout_storage(data)?;

    let Some(mut plan) = load_proxy_upgrade_rollout_plan(data).await? else {
        return Ok(None);
    };
    let restored_node_ids = plan.skipped_node_ids.clone();
    let now = now_unix_secs();

    if restored_node_ids.is_empty() {
        let snapshot = build_rollout_snapshot(&plan, data.list_proxy_nodes().await?, now);
        return Ok(Some(ProxyUpgradeRolloutSkippedRestoreSummary {
            version: plan.version,
            restored_node_ids,
            skipped_node_ids: snapshot.skipped,
            updated: 0,
            blocked: !snapshot.pending.is_empty() || !snapshot.pending_conflicts.is_empty(),
            pending_node_ids: if snapshot.pending_conflicts.is_empty() {
                snapshot.pending
            } else {
                snapshot.pending_conflicts
            },
            rollout_active: true,
            completed: snapshot.completed.len(),
            remaining: snapshot.remaining_total,
        }));
    }

    plan.skipped_node_ids.clear();
    plan.updated_at_unix_secs = now;
    save_proxy_upgrade_rollout_plan(data, &plan).await?;

    let rollout = advance_proxy_upgrade_rollout(data).await?;
    Ok(Some(ProxyUpgradeRolloutSkippedRestoreSummary {
        version: if rollout.version.is_empty() {
            plan.version
        } else {
            rollout.version
        },
        restored_node_ids,
        skipped_node_ids: Vec::new(),
        updated: rollout.updated,
        blocked: rollout.blocked,
        pending_node_ids: rollout.pending_node_ids,
        rollout_active: rollout.rollout_active,
        completed: rollout.completed,
        remaining: rollout.remaining,
    }))
}

pub(crate) async fn retry_proxy_upgrade_rollout_node(
    data: &GatewayDataState,
    node_id: &str,
) -> Result<Option<ProxyUpgradeRolloutNodeActionSummary>, DataLayerError> {
    ensure_proxy_upgrade_rollout_storage(data)?;

    let Some(mut plan) = load_proxy_upgrade_rollout_plan(data).await? else {
        return Ok(None);
    };
    let Some(_node) = data.find_proxy_node(node_id).await? else {
        return Ok(None);
    };
    let now = now_unix_secs();

    let _ = data
        .update_proxy_node_remote_config(&ProxyNodeRemoteConfigMutation {
            node_id: node_id.to_string(),
            node_name: None,
            allowed_ports: None,
            log_level: None,
            heartbeat_interval: None,
            scheduling_state: None,
            upgrade_to: Some(Some(plan.version.clone())),
        })
        .await?;

    plan.skipped_node_ids.retain(|id| id != node_id);
    plan.tracked_nodes
        .retain(|tracked| tracked.node_id != node_id);
    plan.tracked_nodes.push(ProxyUpgradeRolloutTrackedNode {
        node_id: node_id.to_string(),
        dispatched_at_unix_secs: now,
        version_confirmed_at_unix_secs: None,
        traffic_confirmed_at_unix_secs: None,
        confirm_failed_requests: None,
        confirm_dns_failures: None,
        confirm_stream_errors: None,
    });
    plan.last_dispatched_at_unix_secs = Some(now);
    plan.updated_at_unix_secs = now;
    save_proxy_upgrade_rollout_plan(data, &plan).await?;

    let rollout = advance_proxy_upgrade_rollout(data).await?;
    Ok(Some(ProxyUpgradeRolloutNodeActionSummary {
        version: if rollout.version.is_empty() {
            plan.version
        } else {
            rollout.version
        },
        node_id: node_id.to_string(),
        skipped_node_ids: plan.skipped_node_ids,
        updated: rollout.updated,
        blocked: rollout.blocked,
        pending_node_ids: rollout.pending_node_ids,
        rollout_active: rollout.rollout_active,
        completed: rollout.completed,
        remaining: rollout.remaining,
    }))
}

async fn advance_proxy_upgrade_rollout(
    data: &GatewayDataState,
) -> Result<ProxyUpgradeRolloutSummary, DataLayerError> {
    let Some(mut plan) = load_proxy_upgrade_rollout_plan(data).await? else {
        return Ok(ProxyUpgradeRolloutSummary::default());
    };
    let now = now_unix_secs();
    let snapshot = build_rollout_snapshot(&plan, data.list_proxy_nodes().await?, now);

    if plan.tracked_nodes != snapshot.tracked_nodes {
        plan.tracked_nodes = snapshot.tracked_nodes.clone();
        plan.updated_at_unix_secs = now;
        save_proxy_upgrade_rollout_plan(data, &plan).await?;
    }

    for node in &snapshot.ready_to_finalize {
        if remote_config_upgrade_target(node.remote_config.as_ref())
            != Some(normalize_rollout_version(plan.version.as_str()))
        {
            continue;
        }
        let _ = data
            .update_proxy_node_remote_config(&ProxyNodeRemoteConfigMutation {
                node_id: node.id.clone(),
                node_name: None,
                allowed_ports: None,
                log_level: None,
                heartbeat_interval: None,
                scheduling_state: None,
                upgrade_to: Some(None),
            })
            .await?;
    }

    let mut summary = ProxyUpgradeRolloutSummary {
        version: plan.version.clone(),
        batch_size: plan.batch_size,
        cooldown_secs: plan.cooldown_secs,
        updated: 0,
        skipped: snapshot.online_eligible_total,
        blocked: false,
        node_ids: Vec::new(),
        pending_node_ids: snapshot.pending.clone(),
        rollout_active: true,
        completed: snapshot.completed.len(),
        remaining: snapshot.remaining_total,
    };

    if !snapshot.pending_conflicts.is_empty() {
        summary.blocked = true;
        summary.pending_node_ids = snapshot.pending_conflicts;
        return Ok(summary);
    }

    if !snapshot.pending.is_empty() {
        summary.blocked = true;
        return Ok(summary);
    }

    if snapshot.remaining_total == 0 {
        data.delete_system_config_value(PROXY_UPGRADE_ROLLOUT_CONFIG_KEY)
            .await?;
        summary.rollout_active = false;
        summary.skipped = 0;
        return Ok(summary);
    }

    let selected = snapshot
        .available
        .into_iter()
        .take(plan.batch_size)
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Ok(summary);
    }

    let mut updated_node_ids = Vec::with_capacity(selected.len());
    for node in selected {
        let Some(updated) = data
            .update_proxy_node_remote_config(&ProxyNodeRemoteConfigMutation {
                node_id: node.id.clone(),
                node_name: None,
                allowed_ports: None,
                log_level: None,
                heartbeat_interval: None,
                scheduling_state: None,
                upgrade_to: Some(Some(plan.version.clone())),
            })
            .await?
        else {
            continue;
        };
        updated_node_ids.push(updated.id);
    }

    plan.last_dispatched_at_unix_secs = Some(now);
    plan.updated_at_unix_secs = now;
    plan.tracked_nodes
        .extend(
            updated_node_ids
                .iter()
                .cloned()
                .map(|node_id| ProxyUpgradeRolloutTrackedNode {
                    node_id,
                    dispatched_at_unix_secs: now,
                    version_confirmed_at_unix_secs: None,
                    traffic_confirmed_at_unix_secs: None,
                    confirm_failed_requests: None,
                    confirm_dns_failures: None,
                    confirm_stream_errors: None,
                }),
        );
    save_proxy_upgrade_rollout_plan(data, &plan).await?;

    summary.updated = updated_node_ids.len();
    summary.skipped = summary.skipped.saturating_sub(summary.updated);
    summary.node_ids = updated_node_ids.clone();
    summary.pending_node_ids = updated_node_ids;
    summary.remaining = summary.remaining.saturating_sub(summary.updated);
    Ok(summary)
}

fn build_rollout_snapshot(
    plan: &ProxyUpgradeRolloutPlan,
    mut nodes: Vec<StoredProxyNode>,
    now_unix_secs: u64,
) -> RolloutSnapshot {
    nodes.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
    let target_version = normalize_rollout_version(plan.version.as_str());
    let skipped_node_ids = plan
        .skipped_node_ids
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let mut snapshot = RolloutSnapshot::default();
    let mut tracked_by_node_id = plan
        .tracked_nodes
        .iter()
        .cloned()
        .map(|tracked| (tracked.node_id.clone(), tracked))
        .collect::<std::collections::BTreeMap<_, _>>();

    for node in nodes {
        if node.is_manual || !node.tunnel_mode {
            continue;
        }
        if node.status.eq_ignore_ascii_case("online") && proxy_node_accepts_new_tunnels(&node) {
            snapshot.online_eligible_total = snapshot.online_eligible_total.saturating_add(1);
        }

        if skipped_node_ids.contains(node.id.as_str()) {
            snapshot.skipped.push(node.id.clone());
            tracked_by_node_id.remove(node.id.as_str());
            continue;
        }

        let reported_version = proxy_reported_version(node.proxy_metadata.as_ref());
        let pending_target = remote_config_upgrade_target(node.remote_config.as_ref());

        if let Some(mut tracked) = tracked_by_node_id.remove(node.id.as_str()) {
            snapshot.remaining_total = snapshot.remaining_total.saturating_add(1);

            if reported_version.as_deref() == Some(target_version.as_str()) {
                if tracked.version_confirmed_at_unix_secs.is_none() {
                    tracked.version_confirmed_at_unix_secs =
                        node.last_heartbeat_at_unix_secs.or(Some(now_unix_secs));
                    tracked.traffic_confirmed_at_unix_secs = None;
                    tracked.confirm_failed_requests = Some(node.failed_requests);
                    tracked.confirm_dns_failures = Some(node.dns_failures);
                    tracked.confirm_stream_errors = Some(node.stream_errors);
                    snapshot.pending.push(node.id.clone());
                    snapshot.tracked_nodes.push(tracked);
                    continue;
                }

                if tracked_node_is_healthy(&tracked, &node, plan.cooldown_secs, now_unix_secs) {
                    snapshot.remaining_total = snapshot.remaining_total.saturating_sub(1);
                    snapshot.completed.push(node.id.clone());
                    snapshot.ready_to_finalize.push(node);
                    continue;
                }

                snapshot.pending.push(node.id.clone());
                snapshot.tracked_nodes.push(tracked);
                continue;
            }

            if tracked.version_confirmed_at_unix_secs.is_some() {
                tracked.version_confirmed_at_unix_secs = None;
                tracked.traffic_confirmed_at_unix_secs = None;
                tracked.confirm_failed_requests = None;
                tracked.confirm_dns_failures = None;
                tracked.confirm_stream_errors = None;
            }

            snapshot.pending.push(node.id.clone());
            snapshot.tracked_nodes.push(tracked);
            continue;
        }

        if reported_version.as_deref() == Some(target_version.as_str()) {
            snapshot.completed.push(node.id.clone());
            continue;
        }
        if let Some(pending_target) = pending_target {
            snapshot.remaining_total = snapshot.remaining_total.saturating_add(1);
            if pending_target == target_version {
                snapshot.pending.push(node.id.clone());
            } else {
                snapshot.pending_conflicts.push(node.id.clone());
            }
            continue;
        }

        snapshot.remaining_total = snapshot.remaining_total.saturating_add(1);
        if node.status.eq_ignore_ascii_case("online") && proxy_node_accepts_new_tunnels(&node) {
            snapshot.available.push(node);
        }
    }

    for tracked in tracked_by_node_id.into_values() {
        if skipped_node_ids.contains(tracked.node_id.as_str()) {
            snapshot.skipped.push(tracked.node_id);
            continue;
        }
        snapshot.remaining_total = snapshot.remaining_total.saturating_add(1);
        snapshot.pending.push(tracked.node_id.clone());
        snapshot.tracked_nodes.push(tracked);
    }

    snapshot.skipped.sort();
    snapshot.skipped.dedup();
    snapshot
}

async fn load_proxy_upgrade_rollout_plan(
    data: &GatewayDataState,
) -> Result<Option<ProxyUpgradeRolloutPlan>, DataLayerError> {
    let Some(value) = data
        .find_system_config_value(PROXY_UPGRADE_ROLLOUT_CONFIG_KEY)
        .await?
    else {
        return Ok(None);
    };
    match serde_json::from_value::<ProxyUpgradeRolloutPlan>(value) {
        Ok(plan) => Ok(Some(plan)),
        Err(err) => {
            warn!(
                event_name = "proxy_upgrade_rollout_plan_invalid",
                log_type = "ops",
                error = %err,
                "gateway found invalid proxy upgrade rollout state; clearing it"
            );
            let _ = data
                .delete_system_config_value(PROXY_UPGRADE_ROLLOUT_CONFIG_KEY)
                .await?;
            Ok(None)
        }
    }
}

async fn save_proxy_upgrade_rollout_plan(
    data: &GatewayDataState,
    plan: &ProxyUpgradeRolloutPlan,
) -> Result<(), DataLayerError> {
    data.upsert_system_config_value(
        PROXY_UPGRADE_ROLLOUT_CONFIG_KEY,
        &serde_json::to_value(plan).unwrap_or(serde_json::Value::Null),
        Some(PROXY_UPGRADE_ROLLOUT_DESCRIPTION),
    )
    .await?;
    Ok(())
}

pub(crate) async fn record_proxy_upgrade_traffic_success(
    data: &GatewayDataState,
    node_id: &str,
) -> Result<bool, DataLayerError> {
    if !data.has_system_config_store() {
        return Ok(false);
    }

    let Some(mut plan) = load_proxy_upgrade_rollout_plan(data).await? else {
        return Ok(false);
    };
    let now = now_unix_secs();

    let Some(tracked) = plan
        .tracked_nodes
        .iter_mut()
        .find(|tracked| tracked.node_id == node_id)
    else {
        return Ok(false);
    };
    let Some(version_confirmed_at_unix_secs) = tracked.version_confirmed_at_unix_secs else {
        return Ok(false);
    };

    if tracked
        .traffic_confirmed_at_unix_secs
        .is_some_and(|confirmed_at| confirmed_at >= version_confirmed_at_unix_secs)
    {
        return Ok(true);
    }

    tracked.traffic_confirmed_at_unix_secs = Some(now.max(version_confirmed_at_unix_secs));
    plan.updated_at_unix_secs = now;
    save_proxy_upgrade_rollout_plan(data, &plan).await?;
    Ok(true)
}

fn resolve_rollout_probe_config(
    existing: Option<&ProxyUpgradeRolloutProbeConfig>,
    requested: Option<ProxyUpgradeRolloutProbeConfig>,
) -> Option<ProxyUpgradeRolloutProbeConfig> {
    match (existing, requested) {
        (_, Some(requested)) => Some(requested),
        (Some(existing), None) => Some(existing.clone()),
        (None, None) => None,
    }
}

fn normalize_rollout_version(version: &str) -> String {
    version
        .trim()
        .strip_prefix("proxy-v")
        .unwrap_or(version.trim())
        .to_ascii_lowercase()
}

fn same_rollout_version(left: &str, right: &str) -> bool {
    normalize_rollout_version(left) == normalize_rollout_version(right)
}

fn tracked_node_is_healthy(
    tracked: &ProxyUpgradeRolloutTrackedNode,
    node: &StoredProxyNode,
    cooldown_secs: u64,
    now_unix_secs: u64,
) -> bool {
    let Some(version_confirmed_at_unix_secs) = tracked.version_confirmed_at_unix_secs else {
        return false;
    };
    let Some(traffic_confirmed_at_unix_secs) = tracked.traffic_confirmed_at_unix_secs else {
        return false;
    };
    if traffic_confirmed_at_unix_secs < version_confirmed_at_unix_secs {
        return false;
    }
    if now_unix_secs < version_confirmed_at_unix_secs.saturating_add(cooldown_secs) {
        return false;
    }
    if !node.status.eq_ignore_ascii_case("online") || !node.tunnel_connected {
        return false;
    }
    if tracked
        .confirm_failed_requests
        .is_some_and(|baseline| node.failed_requests > baseline)
    {
        return false;
    }
    if tracked
        .confirm_dns_failures
        .is_some_and(|baseline| node.dns_failures > baseline)
    {
        return false;
    }
    if tracked
        .confirm_stream_errors
        .is_some_and(|baseline| node.stream_errors > baseline)
    {
        return false;
    }
    true
}

fn build_rollout_tracked_node_status(
    tracked: &ProxyUpgradeRolloutTrackedNode,
    node: Option<&StoredProxyNode>,
    cooldown_secs: u64,
    now_unix_secs: u64,
) -> ProxyUpgradeRolloutTrackedNodeStatus {
    let state = match tracked.version_confirmed_at_unix_secs {
        None => ProxyUpgradeRolloutTrackedNodeState::AwaitingVersion,
        Some(version_confirmed_at_unix_secs) => {
            if tracked.traffic_confirmed_at_unix_secs.is_none() {
                ProxyUpgradeRolloutTrackedNodeState::AwaitingTraffic
            } else if node.is_some_and(|node| {
                tracked
                    .confirm_failed_requests
                    .is_some_and(|baseline| node.failed_requests > baseline)
                    || tracked
                        .confirm_dns_failures
                        .is_some_and(|baseline| node.dns_failures > baseline)
                    || tracked
                        .confirm_stream_errors
                        .is_some_and(|baseline| node.stream_errors > baseline)
                    || !node.status.eq_ignore_ascii_case("online")
                    || !node.tunnel_connected
            }) {
                ProxyUpgradeRolloutTrackedNodeState::Unhealthy
            } else if now_unix_secs < version_confirmed_at_unix_secs.saturating_add(cooldown_secs) {
                ProxyUpgradeRolloutTrackedNodeState::CoolingDown
            } else {
                ProxyUpgradeRolloutTrackedNodeState::ReadyToFinalize
            }
        }
    };
    let cooldown_remaining_secs = tracked.version_confirmed_at_unix_secs.map(|confirmed_at| {
        confirmed_at
            .saturating_add(cooldown_secs)
            .saturating_sub(now_unix_secs)
    });

    ProxyUpgradeRolloutTrackedNodeStatus {
        node_id: tracked.node_id.clone(),
        state,
        dispatched_at_unix_secs: tracked.dispatched_at_unix_secs,
        version_confirmed_at_unix_secs: tracked.version_confirmed_at_unix_secs,
        traffic_confirmed_at_unix_secs: tracked.traffic_confirmed_at_unix_secs,
        cooldown_remaining_secs,
    }
}

fn ensure_proxy_upgrade_rollout_storage(data: &GatewayDataState) -> Result<(), DataLayerError> {
    if data.has_system_config_store() {
        Ok(())
    } else {
        Err(DataLayerError::InvalidConfiguration(
            "proxy upgrade rollout requires system config storage".to_string(),
        ))
    }
}
