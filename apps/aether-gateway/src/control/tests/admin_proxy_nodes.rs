use http::Uri;

use super::{classify_control_route, headers};

#[test]
fn classifies_admin_proxy_nodes_list_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/proxy-nodes?status=online&skip=10&limit=20"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("proxy_nodes_manage"));
    assert_eq!(decision.route_kind.as_deref(), Some("list_nodes"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:proxy_nodes")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_proxy_nodes_register_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/proxy-nodes/register"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::POST, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("proxy_nodes_manage"));
    assert_eq!(decision.route_kind.as_deref(), Some("register_node"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:proxy_nodes")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_proxy_nodes_events_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/proxy-nodes/node-1/events?limit=50"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("proxy_nodes_manage"));
    assert_eq!(decision.route_kind.as_deref(), Some("list_node_events"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:proxy_nodes")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_proxy_nodes_upgrade_cancel_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/proxy-nodes/upgrade/cancel"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::POST, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("proxy_nodes_manage"));
    assert_eq!(
        decision.route_kind.as_deref(),
        Some("cancel_upgrade_rollout")
    );
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:proxy_nodes")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_proxy_nodes_upgrade_clear_conflicts_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/proxy-nodes/upgrade/clear-conflicts"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::POST, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("proxy_nodes_manage"));
    assert_eq!(
        decision.route_kind.as_deref(),
        Some("clear_upgrade_rollout_conflicts")
    );
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:proxy_nodes")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_proxy_nodes_upgrade_restore_skipped_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/proxy-nodes/upgrade/restore-skipped"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::POST, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("proxy_nodes_manage"));
    assert_eq!(
        decision.route_kind.as_deref(),
        Some("restore_skipped_upgrade_rollout_nodes")
    );
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:proxy_nodes")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_proxy_nodes_upgrade_skip_node_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/proxy-nodes/node-1/upgrade/skip"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::POST, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("proxy_nodes_manage"));
    assert_eq!(
        decision.route_kind.as_deref(),
        Some("skip_upgrade_rollout_node")
    );
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:proxy_nodes")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_proxy_nodes_upgrade_retry_node_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/proxy-nodes/node-1/upgrade/retry"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::POST, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("proxy_nodes_manage"));
    assert_eq!(
        decision.route_kind.as_deref(),
        Some("retry_upgrade_rollout_node")
    );
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:proxy_nodes")
    );
    assert!(!decision.is_execution_runtime_candidate());
}
