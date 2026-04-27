use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aether_data::repository::proxy_nodes::{
    InMemoryProxyNodeRepository, ProxyNodeHeartbeatMutation, ProxyNodeReadRepository,
    ProxyNodeWriteRepository, StoredProxyNode,
};
use aether_runtime::bounded_queue;
use axum::extract::ws::Message;
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde_json::json;
use tokio::sync::watch;

use super::{
    advance_proxy_upgrade_rollout_once, cleanup_audit_logs_with, cleanup_stale_proxy_nodes_once,
    inspect_proxy_upgrade_rollout, next_daily_run_after, next_db_maintenance_run_after,
    next_stats_aggregation_run_after, next_stats_hourly_aggregation_run_after,
    pending_cleanup_batch_size, pending_cleanup_timeout_minutes, plan_pending_cleanup_batch,
    provider_checkin_schedule, record_proxy_upgrade_traffic_success, run_db_maintenance_with,
    run_proxy_upgrade_rollout_once, spawn_audit_cleanup_worker, spawn_db_maintenance_worker,
    spawn_pending_cleanup_worker, spawn_pool_monitor_worker, spawn_pool_quota_probe_worker,
    spawn_provider_checkin_worker, spawn_proxy_node_stale_cleanup_worker,
    spawn_proxy_upgrade_rollout_worker, spawn_stats_aggregation_worker,
    spawn_stats_hourly_aggregation_worker, spawn_usage_cleanup_worker,
    spawn_wallet_daily_usage_aggregation_worker, start_proxy_upgrade_rollout,
    stats_aggregation_target_day, stats_hourly_aggregation_target_hour, summarize_postgres_pool,
    usage_cleanup_settings, usage_cleanup_window, wallet_daily_usage_aggregation_target, AppState,
    DbMaintenanceRunSummary, FailedPendingUsageRow, GatewayDataState,
    ProxyUpgradeRolloutProbeConfig, StalePendingUsageRow, UsageCleanupSettings,
    DELETE_STALE_WALLET_DAILY_USAGE_LEDGERS_SQL, SELECT_STALE_PENDING_USAGE_BATCH_SQL,
    UPDATE_FAILED_VOID_STALE_USAGE_SQL, UPSERT_WALLET_DAILY_USAGE_LEDGER_SQL, USAGE_CLEANUP_HOUR,
    USAGE_CLEANUP_MINUTE, WALLET_DAILY_USAGE_AGGREGATION_HOUR,
    WALLET_DAILY_USAGE_AGGREGATION_MINUTE,
};

#[tokio::test]
async fn spawn_audit_cleanup_worker_skips_when_postgres_unavailable() {
    assert!(spawn_audit_cleanup_worker(Arc::new(GatewayDataState::disabled())).is_none());
}

#[tokio::test]
async fn spawn_db_maintenance_worker_skips_when_postgres_unavailable() {
    assert!(spawn_db_maintenance_worker(Arc::new(GatewayDataState::disabled())).is_none());
}

#[tokio::test]
async fn spawn_pending_cleanup_worker_skips_when_postgres_unavailable() {
    assert!(spawn_pending_cleanup_worker(Arc::new(GatewayDataState::disabled())).is_none());
}

#[tokio::test]
async fn spawn_proxy_node_stale_cleanup_worker_skips_when_proxy_nodes_unavailable() {
    assert!(
        spawn_proxy_node_stale_cleanup_worker(Arc::new(GatewayDataState::disabled())).is_none()
    );
}

#[tokio::test]
async fn spawn_proxy_upgrade_rollout_worker_skips_when_proxy_nodes_unavailable() {
    let state = AppState::new()
        .expect("gateway state should build")
        .with_data_state_for_tests(GatewayDataState::disabled());
    assert!(spawn_proxy_upgrade_rollout_worker(state).is_none());
}

#[tokio::test]
async fn spawn_proxy_upgrade_rollout_worker_skips_when_system_config_unavailable() {
    let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![]));
    let data = GatewayDataState::with_proxy_node_repository_for_tests(repository);
    let state = AppState::new()
        .expect("gateway state should build")
        .with_data_state_for_tests(data);
    assert!(spawn_proxy_upgrade_rollout_worker(state).is_none());
}

#[tokio::test]
async fn spawn_pool_monitor_worker_skips_when_postgres_unavailable() {
    assert!(spawn_pool_monitor_worker(Arc::new(GatewayDataState::disabled())).is_none());
}

#[tokio::test]
async fn spawn_pool_quota_probe_worker_skips_when_provider_catalog_unavailable() {
    let state = AppState::new()
        .expect("gateway state should build")
        .with_data_state_for_tests(GatewayDataState::disabled());

    assert!(spawn_pool_quota_probe_worker(state).is_none());
}

#[test]
fn wallet_daily_usage_queries_use_settlement_snapshots_for_wallet_identity() {
    assert!(UPSERT_WALLET_DAILY_USAGE_LEDGER_SQL.contains("JOIN usage_settlement_snapshots"));
    assert!(UPSERT_WALLET_DAILY_USAGE_LEDGER_SQL.contains("usage_settlement_snapshots.wallet_id"));
    assert!(DELETE_STALE_WALLET_DAILY_USAGE_LEDGERS_SQL.contains("JOIN usage_settlement_snapshots"));
    assert!(DELETE_STALE_WALLET_DAILY_USAGE_LEDGERS_SQL
        .contains("usage_settlement_snapshots.wallet_id = ledgers.wallet_id"));
}

#[test]
fn pending_cleanup_queries_use_settlement_snapshots_for_billing_authority() {
    assert!(SELECT_STALE_PENDING_USAGE_BATCH_SQL.contains("LEFT JOIN usage_settlement_snapshots"));
    assert!(SELECT_STALE_PENDING_USAGE_BATCH_SQL
        .contains("COALESCE(usage_settlement_snapshots.billing_status, usage.billing_status)"));
    assert!(UPDATE_FAILED_VOID_STALE_USAGE_SQL.contains("INSERT INTO usage_settlement_snapshots"));
    assert!(UPDATE_FAILED_VOID_STALE_USAGE_SQL.contains("billing_status = EXCLUDED.billing_status"));
}

fn sample_connected_proxy_node(
    node_id: &str,
    heartbeat_interval: i32,
    last_heartbeat_at_unix_secs: u64,
) -> StoredProxyNode {
    StoredProxyNode::new(
        node_id.to_string(),
        format!("proxy-{node_id}"),
        "127.0.0.1".to_string(),
        0,
        false,
        "online".to_string(),
        heartbeat_interval,
        3,
        10,
        0,
        0,
        0,
        true,
        true,
        0,
    )
    .expect("node should build")
    .with_runtime_fields(
        Some("test".to_string()),
        None,
        Some(last_heartbeat_at_unix_secs),
        None,
        None,
        None,
        None,
        Some(last_heartbeat_at_unix_secs),
        None,
        Some(last_heartbeat_at_unix_secs),
        Some(last_heartbeat_at_unix_secs),
    )
}

#[tokio::test]
async fn stale_proxy_node_cleanup_marks_timed_out_tunnel_offline() {
    let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![
        sample_connected_proxy_node("node-stale", 30, 1),
    ]));
    let data = GatewayDataState::with_proxy_node_repository_for_tests(Arc::clone(&repository));

    let updated = cleanup_stale_proxy_nodes_once(&data)
        .await
        .expect("cleanup should succeed");

    assert_eq!(updated, 1);
    let node = repository
        .find_proxy_node("node-stale")
        .await
        .expect("lookup should succeed")
        .expect("node should exist");
    assert_eq!(node.status, "offline");
    assert_eq!(node.tunnel_connected, false);
    assert_eq!(node.active_connections, 0);
}

#[tokio::test]
async fn proxy_upgrade_rollout_advances_next_wave_after_version_health_confirmation() {
    let mut alpha = sample_connected_proxy_node("node-alpha", 30, 1_800_000_000);
    alpha.name = "alpha".to_string();
    alpha.proxy_metadata = Some(json!({"version": "1.0.0"}));
    alpha.remote_config = None;
    alpha.config_version = 0;

    let mut zeta = sample_connected_proxy_node("node-zeta", 30, 1_800_000_000);
    zeta.name = "zeta".to_string();
    zeta.proxy_metadata = Some(json!({"version": "1.0.0"}));
    zeta.remote_config = None;
    zeta.config_version = 0;

    let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![zeta, alpha]));
    let data = GatewayDataState::with_proxy_node_repository_for_tests(Arc::clone(&repository))
        .with_system_config_values_for_tests(Vec::<(String, serde_json::Value)>::new());

    let first_wave = start_proxy_upgrade_rollout(&data, "2.0.0".to_string(), 1, 0, None)
        .await
        .expect("rollout should start");
    assert_eq!(first_wave.updated, 1);
    assert_eq!(first_wave.node_ids, vec!["node-alpha".to_string()]);
    assert!(first_wave.rollout_active);

    let alpha_after_first = repository
        .find_proxy_node("node-alpha")
        .await
        .expect("lookup should succeed")
        .expect("alpha should exist");
    assert_eq!(
        alpha_after_first
            .remote_config
            .as_ref()
            .and_then(|value| value.get("upgrade_to")),
        Some(&json!("2.0.0"))
    );

    repository
        .apply_heartbeat(&ProxyNodeHeartbeatMutation {
            node_id: "node-alpha".to_string(),
            heartbeat_interval: None,
            active_connections: Some(2),
            total_requests_delta: Some(1),
            avg_latency_ms: Some(2.0),
            failed_requests_delta: Some(0),
            dns_failures_delta: Some(0),
            stream_errors_delta: Some(0),
            proxy_metadata: Some(json!({"version": "2.0.0"})),
            proxy_version: Some("2.0.0".to_string()),
        })
        .await
        .expect("heartbeat should succeed");

    let observed = advance_proxy_upgrade_rollout_once(&data)
        .await
        .expect("rollout should observe confirmed version");
    assert_eq!(observed.updated, 0);
    assert!(observed.blocked);
    assert_eq!(observed.pending_node_ids, vec!["node-alpha".to_string()]);

    assert!(!record_proxy_upgrade_traffic_success(&data, "node-zeta")
        .await
        .expect("untracked node traffic should be ignored"));
    assert!(record_proxy_upgrade_traffic_success(&data, "node-alpha")
        .await
        .expect("traffic confirmation should be recorded"));

    let second_wave = advance_proxy_upgrade_rollout_once(&data)
        .await
        .expect("rollout should advance after a healthy observation cycle");
    assert_eq!(second_wave.updated, 1);
    assert_eq!(second_wave.node_ids, vec!["node-zeta".to_string()]);
    assert!(second_wave.rollout_active);

    repository
        .apply_heartbeat(&ProxyNodeHeartbeatMutation {
            node_id: "node-zeta".to_string(),
            heartbeat_interval: None,
            active_connections: Some(2),
            total_requests_delta: Some(1),
            avg_latency_ms: Some(2.0),
            failed_requests_delta: Some(0),
            dns_failures_delta: Some(0),
            stream_errors_delta: Some(0),
            proxy_metadata: Some(json!({"version": "2.0.0"})),
            proxy_version: Some("proxy-v2.0.0".to_string()),
        })
        .await
        .expect("heartbeat should succeed");

    let zeta_observed = advance_proxy_upgrade_rollout_once(&data)
        .await
        .expect("rollout should observe second wave");
    assert_eq!(zeta_observed.updated, 0);
    assert!(zeta_observed.blocked);
    assert_eq!(
        zeta_observed.pending_node_ids,
        vec!["node-zeta".to_string()]
    );

    assert!(record_proxy_upgrade_traffic_success(&data, "node-zeta")
        .await
        .expect("traffic confirmation should be recorded"));

    let finished = advance_proxy_upgrade_rollout_once(&data)
        .await
        .expect("rollout should finish after the second healthy observation cycle");
    assert!(!finished.rollout_active);
    assert_eq!(finished.completed, 2);
    assert_eq!(finished.remaining, 0);
    assert!(data
        .list_system_config_entries()
        .await
        .expect("system config list should succeed")
        .is_empty());
}

#[tokio::test]
async fn proxy_upgrade_rollout_excludes_draining_nodes_from_online_eligible_pool() {
    let mut alpha = sample_connected_proxy_node("node-alpha", 30, 1_800_000_000);
    alpha.name = "alpha".to_string();
    alpha.proxy_metadata = Some(json!({"version": "1.0.0"}));
    alpha.remote_config = Some(json!({"scheduling_state": "draining"}));
    alpha.config_version = 1;

    let mut zeta = sample_connected_proxy_node("node-zeta", 30, 1_800_000_000);
    zeta.name = "zeta".to_string();
    zeta.proxy_metadata = Some(json!({"version": "1.0.0"}));
    zeta.remote_config = None;
    zeta.config_version = 0;

    let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![zeta, alpha]));
    let data = GatewayDataState::with_proxy_node_repository_for_tests(Arc::clone(&repository))
        .with_system_config_values_for_tests(Vec::<(String, serde_json::Value)>::new());

    let rollout = start_proxy_upgrade_rollout(&data, "2.0.0".to_string(), 2, 0, None)
        .await
        .expect("rollout should start");
    assert_eq!(rollout.updated, 1);
    assert_eq!(rollout.node_ids, vec!["node-zeta".to_string()]);

    let alpha_after = repository
        .find_proxy_node("node-alpha")
        .await
        .expect("lookup should succeed")
        .expect("alpha should exist");
    assert_eq!(
        alpha_after
            .remote_config
            .as_ref()
            .and_then(|value| value.get("upgrade_to")),
        None
    );

    let rollout_status = inspect_proxy_upgrade_rollout(&data)
        .await
        .expect("inspect should succeed")
        .expect("rollout should exist");
    assert_eq!(rollout_status.online_eligible_total, 1);
}

#[tokio::test]
async fn proxy_upgrade_rollout_blocks_next_wave_after_post_upgrade_transport_errors() {
    let mut alpha = sample_connected_proxy_node("node-alpha", 30, 1_800_000_000);
    alpha.name = "alpha".to_string();
    alpha.proxy_metadata = Some(json!({"version": "1.0.0"}));
    alpha.remote_config = None;
    alpha.config_version = 0;

    let mut zeta = sample_connected_proxy_node("node-zeta", 30, 1_800_000_000);
    zeta.name = "zeta".to_string();
    zeta.proxy_metadata = Some(json!({"version": "1.0.0"}));
    zeta.remote_config = None;
    zeta.config_version = 0;

    let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![zeta, alpha]));
    let data = GatewayDataState::with_proxy_node_repository_for_tests(Arc::clone(&repository))
        .with_system_config_values_for_tests(Vec::<(String, serde_json::Value)>::new());

    let first_wave = start_proxy_upgrade_rollout(&data, "2.0.0".to_string(), 1, 0, None)
        .await
        .expect("rollout should start");
    assert_eq!(first_wave.updated, 1);
    assert_eq!(first_wave.node_ids, vec!["node-alpha".to_string()]);

    repository
        .apply_heartbeat(&ProxyNodeHeartbeatMutation {
            node_id: "node-alpha".to_string(),
            heartbeat_interval: None,
            active_connections: Some(2),
            total_requests_delta: Some(1),
            avg_latency_ms: Some(2.0),
            failed_requests_delta: Some(0),
            dns_failures_delta: Some(0),
            stream_errors_delta: Some(0),
            proxy_metadata: Some(json!({"version": "2.0.0"})),
            proxy_version: Some("2.0.0".to_string()),
        })
        .await
        .expect("heartbeat should succeed");

    let observed = advance_proxy_upgrade_rollout_once(&data)
        .await
        .expect("rollout should observe the first upgraded node");
    assert!(observed.blocked);
    assert_eq!(observed.pending_node_ids, vec!["node-alpha".to_string()]);

    assert!(record_proxy_upgrade_traffic_success(&data, "node-alpha")
        .await
        .expect("traffic confirmation should be recorded"));

    tokio::time::sleep(Duration::from_millis(5)).await;

    repository
        .apply_heartbeat(&ProxyNodeHeartbeatMutation {
            node_id: "node-alpha".to_string(),
            heartbeat_interval: None,
            active_connections: Some(2),
            total_requests_delta: Some(1),
            avg_latency_ms: Some(2.0),
            failed_requests_delta: Some(1),
            dns_failures_delta: Some(0),
            stream_errors_delta: Some(0),
            proxy_metadata: Some(json!({"version": "2.0.0"})),
            proxy_version: Some("2.0.0".to_string()),
        })
        .await
        .expect("heartbeat should succeed");

    let blocked = advance_proxy_upgrade_rollout_once(&data)
        .await
        .expect("rollout should stay blocked after post-upgrade transport errors");
    assert_eq!(blocked.updated, 0);
    assert!(blocked.blocked);
    assert_eq!(blocked.pending_node_ids, vec!["node-alpha".to_string()]);

    let zeta_after = repository
        .find_proxy_node("node-zeta")
        .await
        .expect("lookup should succeed")
        .expect("zeta should exist");
    assert!(zeta_after.remote_config.is_none());
}

#[tokio::test]
async fn proxy_upgrade_rollout_active_probe_advances_next_wave_after_version_confirmation() {
    let mut alpha = sample_connected_proxy_node("node-alpha", 30, 1_800_000_000);
    alpha.name = "alpha".to_string();
    alpha.proxy_metadata = Some(json!({"version": "1.0.0"}));
    alpha.remote_config = None;
    alpha.config_version = 0;

    let mut zeta = sample_connected_proxy_node("node-zeta", 30, 1_800_000_000);
    zeta.name = "zeta".to_string();
    zeta.proxy_metadata = Some(json!({"version": "1.0.0"}));
    zeta.remote_config = None;
    zeta.config_version = 0;

    let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![zeta, alpha]));
    let data = GatewayDataState::with_proxy_node_repository_for_tests(Arc::clone(&repository))
        .with_system_config_values_for_tests(Vec::<(String, serde_json::Value)>::new());
    let state = AppState::new()
        .expect("gateway state should build")
        .with_data_state_for_tests(data.clone());

    let first_wave = start_proxy_upgrade_rollout(
        &data,
        "2.0.0".to_string(),
        1,
        0,
        Some(ProxyUpgradeRolloutProbeConfig {
            url: "https://probe.example/health".to_string(),
            timeout_secs: 5,
        }),
    )
    .await
    .expect("rollout should start");
    assert_eq!(first_wave.node_ids, vec!["node-alpha".to_string()]);

    repository
        .apply_heartbeat(&ProxyNodeHeartbeatMutation {
            node_id: "node-alpha".to_string(),
            heartbeat_interval: None,
            active_connections: Some(2),
            total_requests_delta: Some(1),
            avg_latency_ms: Some(2.0),
            failed_requests_delta: Some(0),
            dns_failures_delta: Some(0),
            stream_errors_delta: Some(0),
            proxy_metadata: Some(json!({"version": "2.0.0"})),
            proxy_version: Some("2.0.0".to_string()),
        })
        .await
        .expect("heartbeat should succeed");

    let tunnel_state = state.tunnel.app_state();
    let (proxy_tx, mut proxy_rx) = bounded_queue(8);
    let (proxy_close_tx, _) = watch::channel(false);
    tunnel_state
        .hub
        .register_proxy(Arc::new(crate::tunnel::TunnelProxyConn::new(
            700,
            "node-alpha".to_string(),
            "Node Alpha".to_string(),
            proxy_tx,
            proxy_close_tx,
            16,
        )));

    let responder_hub = tunnel_state.hub.clone();
    let responder = tokio::spawn(async move {
        let request_headers = match proxy_rx.recv().await.expect("headers frame should arrive") {
            Message::Binary(data) => data,
            other => panic!("unexpected message: {other:?}"),
        };
        let request_header = crate::tunnel::tunnel_protocol::FrameHeader::parse(&request_headers)
            .expect("probe request headers should parse");
        assert_eq!(
            request_header.msg_type,
            crate::tunnel::tunnel_protocol::REQUEST_HEADERS
        );

        let request_body = match proxy_rx.recv().await.expect("body frame should arrive") {
            Message::Binary(data) => data,
            other => panic!("unexpected message: {other:?}"),
        };
        let request_body_header = crate::tunnel::tunnel_protocol::FrameHeader::parse(&request_body)
            .expect("probe request body should parse");
        assert_eq!(
            request_body_header.msg_type,
            crate::tunnel::tunnel_protocol::REQUEST_BODY
        );

        let response_meta = crate::tunnel::tunnel_protocol::ResponseMeta {
            status: 204,
            headers: vec![],
        };
        let response_payload =
            serde_json::to_vec(&response_meta).expect("response meta should serialize");
        let mut response_headers_frame = crate::tunnel::tunnel_protocol::encode_frame(
            request_header.stream_id,
            crate::tunnel::tunnel_protocol::RESPONSE_HEADERS,
            0,
            &response_payload,
        );
        responder_hub
            .handle_proxy_frame(700, &mut response_headers_frame)
            .await;

        let mut response_end_frame = crate::tunnel::tunnel_protocol::encode_frame(
            request_header.stream_id,
            crate::tunnel::tunnel_protocol::STREAM_END,
            0,
            &[],
        );
        responder_hub
            .handle_proxy_frame(700, &mut response_end_frame)
            .await;
    });

    run_proxy_upgrade_rollout_once(&state)
        .await
        .expect("rollout worker should succeed");
    responder.await.expect("probe responder should complete");

    let zeta_after = repository
        .find_proxy_node("node-zeta")
        .await
        .expect("lookup should succeed")
        .expect("zeta should exist");
    assert_eq!(
        zeta_after
            .remote_config
            .as_ref()
            .and_then(|value| value.get("upgrade_to")),
        Some(&json!("2.0.0"))
    );
}

#[tokio::test]
async fn spawn_stats_aggregation_worker_skips_when_postgres_unavailable() {
    assert!(spawn_stats_aggregation_worker(Arc::new(GatewayDataState::disabled())).is_none());
}

#[tokio::test]
async fn spawn_stats_hourly_aggregation_worker_skips_when_postgres_unavailable() {
    assert!(
        spawn_stats_hourly_aggregation_worker(Arc::new(GatewayDataState::disabled())).is_none()
    );
}

#[tokio::test]
async fn spawn_usage_cleanup_worker_skips_when_postgres_unavailable() {
    assert!(spawn_usage_cleanup_worker(Arc::new(GatewayDataState::disabled())).is_none());
}

#[tokio::test]
async fn spawn_wallet_daily_usage_aggregation_worker_skips_when_postgres_unavailable() {
    assert!(
        spawn_wallet_daily_usage_aggregation_worker(Arc::new(GatewayDataState::disabled()))
            .is_none()
    );
}

#[tokio::test]
async fn spawn_provider_checkin_worker_skips_when_provider_catalog_unavailable() {
    let state = AppState::new()
        .expect("gateway state should build")
        .with_data_state_for_tests(GatewayDataState::disabled());

    assert!(spawn_provider_checkin_worker(state).is_none());
}

#[tokio::test]
async fn cleanup_audit_logs_respects_auto_cleanup_toggle() {
    let data = GatewayDataState::disabled()
        .with_system_config_values_for_tests([("enable_auto_cleanup".to_string(), json!(false))]);

    let deleted = cleanup_audit_logs_with(&data, |_cutoff_time, _delete_limit| async move {
        panic!("audit cleanup should not run when auto cleanup is disabled");
        #[allow(unreachable_code)]
        Ok(0)
    })
    .await
    .expect("audit cleanup should short-circuit");

    assert_eq!(deleted, 0);
}

#[tokio::test]
async fn cleanup_audit_logs_uses_retention_and_batch_settings() {
    let data = GatewayDataState::disabled().with_system_config_values_for_tests([
        ("enable_auto_cleanup".to_string(), json!(true)),
        ("audit_log_retention_days".to_string(), json!(21)),
        ("cleanup_batch_size".to_string(), json!(2)),
    ]);
    let observed_limits = Arc::new(Mutex::new(Vec::new()));
    let observed_cutoffs = Arc::new(Mutex::new(Vec::new()));
    let batch_results = Arc::new(Mutex::new(VecDeque::from([2usize, 1usize])));
    let started_at = Utc::now();

    let deleted = cleanup_audit_logs_with(&data, {
        let observed_limits = Arc::clone(&observed_limits);
        let observed_cutoffs = Arc::clone(&observed_cutoffs);
        let batch_results = Arc::clone(&batch_results);
        move |cutoff_time, delete_limit| {
            observed_limits
                .lock()
                .expect("observed limits lock")
                .push(delete_limit);
            observed_cutoffs
                .lock()
                .expect("observed cutoffs lock")
                .push(cutoff_time);
            let next = batch_results
                .lock()
                .expect("batch results lock")
                .pop_front()
                .unwrap_or_default();
            async move { Ok(next) }
        }
    })
    .await
    .expect("audit cleanup should succeed");
    let finished_at = Utc::now();

    assert_eq!(deleted, 3);
    assert_eq!(
        *observed_limits.lock().expect("observed limits lock"),
        vec![2, 2]
    );
    let observed_cutoffs = observed_cutoffs.lock().expect("observed cutoffs lock");
    assert_eq!(observed_cutoffs.len(), 2);
    let earliest_expected = started_at - chrono::Duration::days(21);
    let latest_expected = finished_at - chrono::Duration::days(21);
    for cutoff_time in observed_cutoffs.iter() {
        assert!(*cutoff_time >= earliest_expected);
        assert!(*cutoff_time <= latest_expected);
    }
}

#[tokio::test]
async fn pending_cleanup_settings_use_timeout_and_cap_batch_size() {
    let data = GatewayDataState::disabled().with_system_config_values_for_tests([
        ("pending_request_timeout_minutes".to_string(), json!(25)),
        ("cleanup_batch_size".to_string(), json!(500)),
    ]);

    let timeout_minutes = pending_cleanup_timeout_minutes(&data)
        .await
        .expect("timeout should resolve");
    let batch_size = pending_cleanup_batch_size(&data)
        .await
        .expect("batch size should resolve");

    assert_eq!(timeout_minutes, 25);
    assert_eq!(batch_size, 200);
}

#[test]
fn pending_cleanup_plan_recovers_completed_requests_and_voids_failed_pending_billing() {
    let plan = plan_pending_cleanup_batch(
        vec![
            StalePendingUsageRow {
                id: "usage-1".to_string(),
                request_id: "req-1".to_string(),
                status: "streaming".to_string(),
                billing_status: "pending".to_string(),
            },
            StalePendingUsageRow {
                id: "usage-2".to_string(),
                request_id: "req-2".to_string(),
                status: "pending".to_string(),
                billing_status: "pending".to_string(),
            },
            StalePendingUsageRow {
                id: "usage-3".to_string(),
                request_id: "req-3".to_string(),
                status: "streaming".to_string(),
                billing_status: "settled".to_string(),
            },
        ],
        &HashSet::from(["req-1".to_string()]),
        10,
    );

    assert_eq!(plan.recovered_usage_ids, vec!["usage-1".to_string()]);
    assert_eq!(plan.recovered_request_ids, vec!["req-1".to_string()]);
    assert_eq!(
        plan.failed_request_ids,
        vec!["req-2".to_string(), "req-3".to_string()]
    );
    assert_eq!(
        plan.failed_usage_rows,
        vec![
            FailedPendingUsageRow {
                id: "usage-2".to_string(),
                error_message: "请求超时: 状态 'pending' 超过 10 分钟未完成".to_string(),
                should_void_billing: true,
            },
            FailedPendingUsageRow {
                id: "usage-3".to_string(),
                error_message: "请求超时: 状态 'streaming' 超过 10 分钟未完成".to_string(),
                should_void_billing: false,
            },
        ]
    );
}

#[tokio::test]
async fn usage_cleanup_settings_resolve_batch_and_delete_toggle() {
    let data = GatewayDataState::disabled().with_system_config_values_for_tests([
        ("detail_log_retention_days".to_string(), json!(7)),
        ("compressed_log_retention_days".to_string(), json!(30)),
        ("header_retention_days".to_string(), json!(90)),
        ("log_retention_days".to_string(), json!(365)),
        ("cleanup_batch_size".to_string(), json!(0)),
        ("auto_delete_expired_keys".to_string(), json!(true)),
    ]);

    let settings = usage_cleanup_settings(&data)
        .await
        .expect("usage cleanup settings should resolve");

    assert_eq!(
        settings,
        UsageCleanupSettings {
            detail_retention_days: 7,
            compressed_retention_days: 30,
            header_retention_days: 90,
            log_retention_days: 365,
            batch_size: 1,
            auto_delete_expired_keys: true,
        }
    );
}

#[test]
fn usage_cleanup_window_uses_non_overlapping_ranges() {
    let now_utc = "2026-03-18T03:00:00Z"
        .parse::<DateTime<Utc>>()
        .expect("timestamp should parse");
    let window = usage_cleanup_window(
        now_utc,
        UsageCleanupSettings {
            detail_retention_days: 7,
            compressed_retention_days: 30,
            header_retention_days: 90,
            log_retention_days: 365,
            batch_size: 123,
            auto_delete_expired_keys: false,
        },
    );

    assert_eq!(
        window.detail_cutoff.to_rfc3339(),
        "2026-03-11T03:00:00+00:00"
    );
    assert_eq!(
        window.compressed_cutoff.to_rfc3339(),
        "2026-02-16T03:00:00+00:00"
    );
    assert_eq!(
        window.header_cutoff.to_rfc3339(),
        "2025-12-18T03:00:00+00:00"
    );
    assert_eq!(window.log_cutoff.to_rfc3339(), "2025-03-18T03:00:00+00:00");
    assert!(window.detail_cutoff > window.compressed_cutoff);
    assert!(window.compressed_cutoff > window.log_cutoff);
}

#[tokio::test]
async fn summarize_postgres_pool_uses_busy_connections_for_usage_rate() {
    let data = GatewayDataState::from_config(crate::data::GatewayDataConfig::from_postgres_config(
        aether_data::postgres::PostgresPoolConfig {
            database_url: "postgres://localhost/aether".to_string(),
            min_connections: 1,
            max_connections: 8,
            acquire_timeout_ms: 1_000,
            idle_timeout_ms: 5_000,
            max_lifetime_ms: 30_000,
            statement_cache_capacity: 64,
            require_ssl: false,
        },
    ))
    .expect("gateway data state should build");

    let summary = summarize_postgres_pool(&data).expect("pool summary should exist");

    assert_eq!(summary.checked_out, 0);
    assert_eq!(summary.pool_size, 0);
    assert_eq!(summary.idle, 0);
    assert_eq!(summary.max_connections, 8);
    assert_eq!(summary.usage_rate, 0.0);
}

#[test]
fn stats_aggregation_target_uses_previous_utc_day() {
    let now_utc = "2026-04-05T10:20:30Z"
        .parse::<DateTime<Utc>>()
        .expect("timestamp should parse");

    let target = stats_aggregation_target_day(now_utc);

    assert_eq!(target.to_rfc3339(), "2026-04-04T00:00:00+00:00");
}

#[test]
fn next_stats_aggregation_run_aligns_to_same_day_when_before_slot() {
    let now_utc = "2026-04-05T00:04:59Z"
        .parse::<DateTime<Utc>>()
        .expect("timestamp should parse");

    let next = next_stats_aggregation_run_after(now_utc);

    assert_eq!(next.to_rfc3339(), "2026-04-05T00:05:00+00:00");
}

#[test]
fn next_stats_aggregation_run_rolls_to_next_day_after_slot() {
    let now_utc = "2026-04-05T00:05:00Z"
        .parse::<DateTime<Utc>>()
        .expect("timestamp should parse");

    let next = next_stats_aggregation_run_after(now_utc);

    assert_eq!(next.to_rfc3339(), "2026-04-06T00:05:00+00:00");
}

#[test]
fn stats_hourly_aggregation_target_uses_previous_utc_hour() {
    let now_utc = "2026-04-05T10:20:30Z"
        .parse::<DateTime<Utc>>()
        .expect("timestamp should parse");

    let target = stats_hourly_aggregation_target_hour(now_utc);

    assert_eq!(target.to_rfc3339(), "2026-04-05T09:00:00+00:00");
}

#[test]
fn next_stats_hourly_aggregation_run_aligns_to_same_hour_when_before_slot() {
    let now_utc = "2026-04-05T10:04:59Z"
        .parse::<DateTime<Utc>>()
        .expect("timestamp should parse");

    let next = next_stats_hourly_aggregation_run_after(now_utc);

    assert_eq!(next.to_rfc3339(), "2026-04-05T10:05:00+00:00");
}

#[test]
fn next_stats_hourly_aggregation_run_rolls_to_next_hour_after_slot() {
    let now_utc = "2026-04-05T10:05:00Z"
        .parse::<DateTime<Utc>>()
        .expect("timestamp should parse");

    let next = next_stats_hourly_aggregation_run_after(now_utc);

    assert_eq!(next.to_rfc3339(), "2026-04-05T11:05:00+00:00");
}

#[tokio::test]
async fn db_maintenance_respects_enable_toggle() {
    let data = GatewayDataState::disabled()
        .with_system_config_values_for_tests([("enable_db_maintenance".to_string(), json!(false))]);

    let summary = run_db_maintenance_with(&data, |_table_name| async move {
        panic!("db maintenance should not run when disabled");
        #[allow(unreachable_code)]
        Ok(())
    })
    .await
    .expect("db maintenance should short-circuit");

    assert_eq!(
        summary,
        DbMaintenanceRunSummary {
            attempted: 0,
            succeeded: 0,
        }
    );
}

#[tokio::test]
async fn db_maintenance_continues_across_table_failures() {
    let data = GatewayDataState::disabled()
        .with_system_config_values_for_tests([("enable_db_maintenance".to_string(), json!(true))]);
    let seen_tables = Arc::new(Mutex::new(Vec::new()));

    let summary = run_db_maintenance_with(&data, {
        let seen_tables = Arc::clone(&seen_tables);
        move |table_name| {
            seen_tables
                .lock()
                .expect("seen tables lock")
                .push(table_name.to_string());
            async move {
                if table_name == "request_candidates" {
                    Err(aether_data::DataLayerError::InvalidInput(
                        "boom".to_string(),
                    ))
                } else {
                    Ok(())
                }
            }
        }
    })
    .await
    .expect("db maintenance should continue after failures");

    assert_eq!(
        summary,
        DbMaintenanceRunSummary {
            attempted: 3,
            succeeded: 2,
        }
    );
    assert_eq!(
        *seen_tables.lock().expect("seen tables lock"),
        vec![
            "usage".to_string(),
            "request_candidates".to_string(),
            "audit_logs".to_string(),
        ]
    );
}

#[test]
fn next_db_maintenance_run_aligns_to_same_week_when_before_slot() {
    let timezone: Tz = "Asia/Shanghai".parse().expect("timezone should parse");
    let now_utc = "2026-04-03T20:59:00Z"
        .parse::<DateTime<Utc>>()
        .expect("timestamp should parse");

    let next = next_db_maintenance_run_after(now_utc, timezone);

    assert_eq!(next.to_rfc3339(), "2026-04-04T21:00:00+00:00");
}

#[test]
fn next_db_maintenance_run_rolls_to_next_week_after_slot() {
    let timezone: Tz = "Asia/Shanghai".parse().expect("timezone should parse");
    let now_utc = "2026-04-04T21:00:01Z"
        .parse::<DateTime<Utc>>()
        .expect("timestamp should parse");

    let next = next_db_maintenance_run_after(now_utc, timezone);

    assert_eq!(next.to_rfc3339(), "2026-04-11T21:00:00+00:00");
}

#[test]
fn wallet_daily_usage_aggregation_target_uses_previous_local_day_window() {
    let timezone: Tz = "Asia/Shanghai".parse().expect("timezone should parse");
    let now_utc = "2026-03-31T16:15:00Z"
        .parse::<DateTime<Utc>>()
        .expect("timestamp should parse");

    let target = wallet_daily_usage_aggregation_target(now_utc, timezone);

    assert_eq!(target.billing_date.to_string(), "2026-03-31");
    assert_eq!(target.billing_timezone, "Asia/Shanghai");
    assert_eq!(
        target.window_start_utc.to_rfc3339(),
        "2026-03-30T16:00:00+00:00"
    );
    assert_eq!(
        target.window_end_utc.to_rfc3339(),
        "2026-03-31T16:00:00+00:00"
    );
}

#[tokio::test]
async fn provider_checkin_schedule_uses_default_for_invalid_value() {
    let data = GatewayDataState::disabled().with_system_config_values_for_tests([(
        "provider_checkin_time".to_string(),
        json!("25:99"),
    )]);

    let schedule = provider_checkin_schedule(&data)
        .await
        .expect("provider checkin schedule should resolve");

    assert_eq!(schedule, (1, 5));
}

#[test]
fn next_provider_checkin_run_aligns_to_same_day_when_before_slot() {
    let timezone: Tz = "Asia/Shanghai".parse().expect("timezone should parse");
    let now_utc = "2026-03-31T16:59:00Z"
        .parse::<DateTime<Utc>>()
        .expect("timestamp should parse");

    let next = next_daily_run_after(now_utc, timezone, 1, 5);

    assert_eq!(next.to_rfc3339(), "2026-03-31T17:05:00+00:00");
}

#[test]
fn next_provider_checkin_run_rolls_to_next_day_after_slot() {
    let timezone: Tz = "Asia/Shanghai".parse().expect("timezone should parse");
    let now_utc = "2026-03-31T17:05:01Z"
        .parse::<DateTime<Utc>>()
        .expect("timestamp should parse");

    let next = next_daily_run_after(now_utc, timezone, 1, 5);

    assert_eq!(next.to_rfc3339(), "2026-04-01T17:05:00+00:00");
}

#[test]
fn next_usage_cleanup_run_aligns_to_same_day_when_before_slot() {
    let timezone: Tz = "Asia/Shanghai".parse().expect("timezone should parse");
    let now_utc = "2026-03-17T18:59:00Z"
        .parse::<DateTime<Utc>>()
        .expect("timestamp should parse");

    let next = next_daily_run_after(now_utc, timezone, USAGE_CLEANUP_HOUR, USAGE_CLEANUP_MINUTE);

    assert_eq!(next.to_rfc3339(), "2026-03-17T19:00:00+00:00");
}

#[test]
fn next_usage_cleanup_run_rolls_to_next_day_after_slot() {
    let timezone: Tz = "Asia/Shanghai".parse().expect("timezone should parse");
    let now_utc = "2026-03-17T19:00:01Z"
        .parse::<DateTime<Utc>>()
        .expect("timestamp should parse");

    let next = next_daily_run_after(now_utc, timezone, USAGE_CLEANUP_HOUR, USAGE_CLEANUP_MINUTE);

    assert_eq!(next.to_rfc3339(), "2026-03-18T19:00:00+00:00");
}

#[test]
fn next_wallet_daily_usage_aggregation_run_aligns_to_same_day_when_before_slot() {
    let timezone: Tz = "Asia/Shanghai".parse().expect("timezone should parse");
    let now_utc = "2026-03-31T16:09:00Z"
        .parse::<DateTime<Utc>>()
        .expect("timestamp should parse");

    let next = next_daily_run_after(
        now_utc,
        timezone,
        WALLET_DAILY_USAGE_AGGREGATION_HOUR,
        WALLET_DAILY_USAGE_AGGREGATION_MINUTE,
    );

    assert_eq!(next.to_rfc3339(), "2026-03-31T16:10:00+00:00");
}

#[test]
fn next_wallet_daily_usage_aggregation_run_rolls_to_next_day_after_slot() {
    let timezone: Tz = "Asia/Shanghai".parse().expect("timezone should parse");
    let now_utc = "2026-03-31T16:10:01Z"
        .parse::<DateTime<Utc>>()
        .expect("timestamp should parse");

    let next = next_daily_run_after(
        now_utc,
        timezone,
        WALLET_DAILY_USAGE_AGGREGATION_HOUR,
        WALLET_DAILY_USAGE_AGGREGATION_MINUTE,
    );

    assert_eq!(next.to_rfc3339(), "2026-04-01T16:10:00+00:00");
}
