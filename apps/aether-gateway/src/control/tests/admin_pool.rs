use http::Uri;

use crate::handlers::shared::local_proxy_route_requires_buffered_body;

use super::{classify_control_route, headers, GatewayPublicRequestContext};

#[test]
fn classifies_admin_pool_overview_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/pool/overview"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("pool_manage"));
    assert_eq!(decision.route_kind.as_deref(), Some("overview"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:pool")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_pool_scheduling_presets_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/pool/scheduling-presets"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("pool_manage"));
    assert_eq!(decision.route_kind.as_deref(), Some("scheduling_presets"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:pool")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_pool_provider_key_routes_as_admin_proxy_route() {
    let headers = headers(&[]);

    let list_uri: Uri = "/api/admin/pool/provider-1/keys?page=1"
        .parse()
        .expect("uri should parse");
    let list = classify_control_route(&http::Method::GET, &list_uri, &headers)
        .expect("route should classify");
    assert_eq!(list.route_family.as_deref(), Some("pool_manage"));
    assert_eq!(list.route_kind.as_deref(), Some("list_keys"));

    let scores_uri: Uri = "/api/admin/pool/provider-1/scores?api_format=openai:responses"
        .parse()
        .expect("uri should parse");
    let scores = classify_control_route(&http::Method::GET, &scores_uri, &headers)
        .expect("route should classify");
    assert_eq!(scores.route_family.as_deref(), Some("pool_manage"));
    assert_eq!(scores.route_kind.as_deref(), Some("scores"));

    let batch_import_uri: Uri = "/api/admin/pool/provider-1/keys/batch-import"
        .parse()
        .expect("uri should parse");
    let batch_import = classify_control_route(&http::Method::POST, &batch_import_uri, &headers)
        .expect("route should classify");
    assert_eq!(batch_import.route_family.as_deref(), Some("pool_manage"));
    assert_eq!(
        batch_import.route_kind.as_deref(),
        Some("batch_import_keys")
    );

    let batch_action_uri: Uri = "/api/admin/pool/provider-1/keys/batch-action"
        .parse()
        .expect("uri should parse");
    let batch_action = classify_control_route(&http::Method::POST, &batch_action_uri, &headers)
        .expect("route should classify");
    assert_eq!(batch_action.route_family.as_deref(), Some("pool_manage"));
    assert_eq!(
        batch_action.route_kind.as_deref(),
        Some("batch_action_keys")
    );

    let selection_snapshot_uri: Uri = "/api/admin/pool/provider-1/keys/selection-snapshot"
        .parse()
        .expect("uri should parse");
    let selection_snapshot =
        classify_control_route(&http::Method::POST, &selection_snapshot_uri, &headers)
            .expect("route should classify");
    assert_eq!(
        selection_snapshot.route_family.as_deref(),
        Some("pool_manage")
    );
    assert_eq!(
        selection_snapshot.route_kind.as_deref(),
        Some("create_selection_snapshot")
    );

    let resolve_selection_uri: Uri = "/api/admin/pool/provider-1/keys/resolve-selection"
        .parse()
        .expect("uri should parse");
    let resolve_selection =
        classify_control_route(&http::Method::POST, &resolve_selection_uri, &headers)
            .expect("route should classify");
    assert_eq!(
        resolve_selection.route_family.as_deref(),
        Some("pool_manage")
    );
    assert_eq!(
        resolve_selection.route_kind.as_deref(),
        Some("resolve_selection")
    );

    let batch_delete_task_uri: Uri = "/api/admin/pool/provider-1/keys/batch-delete-task/task-1"
        .parse()
        .expect("uri should parse");
    let batch_delete_task =
        classify_control_route(&http::Method::GET, &batch_delete_task_uri, &headers)
            .expect("route should classify");
    assert_eq!(
        batch_delete_task.route_family.as_deref(),
        Some("pool_manage")
    );
    assert_eq!(
        batch_delete_task.route_kind.as_deref(),
        Some("batch_delete_task_status")
    );

    let cleanup_banned_uri: Uri = "/api/admin/pool/provider-1/keys/cleanup-banned"
        .parse()
        .expect("uri should parse");
    let cleanup_banned = classify_control_route(&http::Method::POST, &cleanup_banned_uri, &headers)
        .expect("route should classify");
    assert_eq!(cleanup_banned.route_family.as_deref(), Some("pool_manage"));
    assert_eq!(
        cleanup_banned.route_kind.as_deref(),
        Some("cleanup_banned_keys")
    );
    assert_eq!(
        cleanup_banned.auth_endpoint_signature.as_deref(),
        Some("admin:pool")
    );
}

#[test]
fn admin_pool_selection_snapshot_buffers_request_body() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/pool/provider-1/keys/selection-snapshot"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::POST, &uri, &headers).expect("route should classify");
    let context = GatewayPublicRequestContext::from_request_parts(
        "trace-pool-selection-snapshot",
        &http::Method::POST,
        &uri,
        &headers,
        Some(decision),
    );

    assert!(local_proxy_route_requires_buffered_body(&context));
}

#[test]
fn classifies_admin_pool_trailing_slash_routes_as_admin_proxy_route() {
    let headers = headers(&[]);

    let list_uri: Uri = "/api/admin/pool/provider-1/keys/"
        .parse()
        .expect("uri should parse");
    let list = classify_control_route(&http::Method::GET, &list_uri, &headers)
        .expect("route should classify");
    assert_eq!(list.route_family.as_deref(), Some("pool_manage"));
    assert_eq!(list.route_kind.as_deref(), Some("list_keys"));

    let selection_snapshot_uri: Uri = "/api/admin/pool/provider-1/keys/selection-snapshot/"
        .parse()
        .expect("uri should parse");
    let selection_snapshot =
        classify_control_route(&http::Method::POST, &selection_snapshot_uri, &headers)
            .expect("route should classify");
    assert_eq!(
        selection_snapshot.route_family.as_deref(),
        Some("pool_manage")
    );
    assert_eq!(
        selection_snapshot.route_kind.as_deref(),
        Some("create_selection_snapshot")
    );

    let resolve_selection_uri: Uri = "/api/admin/pool/provider-1/keys/resolve-selection/"
        .parse()
        .expect("uri should parse");
    let resolve_selection =
        classify_control_route(&http::Method::POST, &resolve_selection_uri, &headers)
            .expect("route should classify");
    assert_eq!(
        resolve_selection.route_family.as_deref(),
        Some("pool_manage")
    );
    assert_eq!(
        resolve_selection.route_kind.as_deref(),
        Some("resolve_selection")
    );
}

#[test]
fn classifies_admin_pool_malformed_provider_id_routes_as_admin_proxy_route() {
    let headers = headers(&[]);

    let list_uri: Uri = "/api/admin/pool//keys".parse().expect("uri should parse");
    let list = classify_control_route(&http::Method::GET, &list_uri, &headers)
        .expect("route should classify");
    assert_eq!(list.route_family.as_deref(), Some("pool_manage"));
    assert_eq!(list.route_kind.as_deref(), Some("list_keys"));

    let import_uri: Uri = "/api/admin/pool//keys/batch-import"
        .parse()
        .expect("uri should parse");
    let import = classify_control_route(&http::Method::POST, &import_uri, &headers)
        .expect("route should classify");
    assert_eq!(import.route_family.as_deref(), Some("pool_manage"));
    assert_eq!(import.route_kind.as_deref(), Some("batch_import_keys"));

    let cleanup_uri: Uri = "/api/admin/pool//keys/cleanup-banned"
        .parse()
        .expect("uri should parse");
    let cleanup = classify_control_route(&http::Method::POST, &cleanup_uri, &headers)
        .expect("route should classify");
    assert_eq!(cleanup.route_family.as_deref(), Some("pool_manage"));
    assert_eq!(cleanup.route_kind.as_deref(), Some("cleanup_banned_keys"));
}
