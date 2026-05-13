use http::Uri;

use crate::control::management_token_required_permission;

use super::{classify_control_route, headers};

#[test]
fn classifies_admin_provider_oauth_start_key_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/provider-oauth/keys/key-123/start"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::POST, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(
        decision.route_family.as_deref(),
        Some("provider_oauth_manage")
    );
    assert_eq!(decision.route_kind.as_deref(), Some("start_key_oauth"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:provider_oauth")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_provider_oauth_start_provider_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/provider-oauth/providers/provider-123/start"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::POST, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(
        decision.route_family.as_deref(),
        Some("provider_oauth_manage")
    );
    assert_eq!(decision.route_kind.as_deref(), Some("start_provider_oauth"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:provider_oauth")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_provider_oauth_batch_import_task_status_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/provider-oauth/providers/provider-123/batch-import/tasks/task-456"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(
        decision.route_family.as_deref(),
        Some("provider_oauth_manage")
    );
    assert_eq!(
        decision.route_kind.as_deref(),
        Some("get_batch_import_task_status")
    );
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:pool")
    );
    assert_eq!(
        management_token_required_permission(&http::Method::GET, &decision).as_deref(),
        Some("admin:pool:read")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_provider_oauth_maintenance_routes_as_admin_proxy_route() {
    let headers = headers(&[]);
    for (method, path, route_kind, expected_signature, expected_required_permission) in [
        (
            http::Method::POST,
            "/api/admin/provider-oauth/keys/key-123/complete",
            "complete_key_oauth",
            "admin:provider_oauth",
            "admin:provider_oauth:write",
        ),
        (
            http::Method::POST,
            "/api/admin/provider-oauth/keys/key-123/refresh",
            "refresh_key_oauth",
            "admin:provider_oauth",
            "admin:provider_oauth:write",
        ),
        (
            http::Method::POST,
            "/api/admin/provider-oauth/providers/provider-123/complete",
            "complete_provider_oauth",
            "admin:provider_oauth",
            "admin:provider_oauth:write",
        ),
        (
            http::Method::POST,
            "/api/admin/provider-oauth/providers/provider-123/import-refresh-token",
            "import_refresh_token",
            "admin:provider_oauth",
            "admin:provider_oauth:write",
        ),
        (
            http::Method::POST,
            "/api/admin/provider-oauth/providers/provider-123/batch-import",
            "batch_import_oauth",
            "admin:pool",
            "admin:pool:write",
        ),
        (
            http::Method::POST,
            "/api/admin/provider-oauth/providers/provider-123/batch-import/tasks",
            "start_batch_import_oauth_task",
            "admin:pool",
            "admin:pool:write",
        ),
        (
            http::Method::GET,
            "/api/admin/provider-oauth/providers/provider-123/batch-import/tasks/task-123",
            "get_batch_import_task_status",
            "admin:pool",
            "admin:pool:read",
        ),
        (
            http::Method::POST,
            "/api/admin/provider-oauth/providers/provider-123/device-authorize",
            "device_authorize",
            "admin:provider_oauth",
            "admin:provider_oauth:write",
        ),
        (
            http::Method::POST,
            "/api/admin/provider-oauth/providers/provider-123/device-poll",
            "device_poll",
            "admin:provider_oauth",
            "admin:provider_oauth:write",
        ),
        (
            http::Method::POST,
            "/api/admin/provider-oauth/keys/key-123/kiro/overage",
            "set_kiro_overage",
        ),
    ] {
        let uri: Uri = path.parse().expect("uri should parse");
        let decision =
            classify_control_route(&method, &uri, &headers).expect("route should classify");

        assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
        assert_eq!(
            decision.route_family.as_deref(),
            Some("provider_oauth_manage")
        );
        assert_eq!(decision.route_kind.as_deref(), Some(route_kind));
        assert_eq!(
            decision.auth_endpoint_signature.as_deref(),
            Some(expected_signature)
        );
        assert_eq!(
            management_token_required_permission(&method, &decision).as_deref(),
            Some(expected_required_permission)
        );
        assert!(!decision.is_execution_runtime_candidate());
    }
}

#[test]
fn classifies_admin_oauth_list_providers_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/oauth/providers?limit=20"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("oauth_manage"));
    assert_eq!(decision.route_kind.as_deref(), Some("list_providers"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:oauth")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_oauth_get_provider_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/oauth/providers/linuxdo"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("oauth_manage"));
    assert_eq!(decision.route_kind.as_deref(), Some("get_provider"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:oauth")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_oauth_upsert_provider_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/oauth/providers/linuxdo"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::PUT, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("oauth_manage"));
    assert_eq!(decision.route_kind.as_deref(), Some("upsert_provider"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:oauth")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_oauth_delete_provider_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/oauth/providers/linuxdo"
        .parse()
        .expect("uri should parse");
    let decision = classify_control_route(&http::Method::DELETE, &uri, &headers)
        .expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("oauth_manage"));
    assert_eq!(decision.route_kind.as_deref(), Some("delete_provider"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:oauth")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_oauth_test_provider_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/oauth/providers/linuxdo/test"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::POST, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("oauth_manage"));
    assert_eq!(decision.route_kind.as_deref(), Some("test_provider"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:oauth")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_get_management_token_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/management-tokens/token-123"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(
        decision.route_family.as_deref(),
        Some("management_tokens_manage")
    );
    assert_eq!(decision.route_kind.as_deref(), Some("get_token"));
}

#[test]
fn classifies_admin_delete_management_token_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/management-tokens/token-123"
        .parse()
        .expect("uri should parse");
    let decision = classify_control_route(&http::Method::DELETE, &uri, &headers)
        .expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(
        decision.route_family.as_deref(),
        Some("management_tokens_manage")
    );
    assert_eq!(decision.route_kind.as_deref(), Some("delete_token"));
}

#[test]
fn classifies_admin_toggle_management_token_status_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/management-tokens/token-123/status"
        .parse()
        .expect("uri should parse");
    let decision = classify_control_route(&http::Method::PATCH, &uri, &headers)
        .expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(
        decision.route_family.as_deref(),
        Some("management_tokens_manage")
    );
    assert_eq!(decision.route_kind.as_deref(), Some("toggle_status"));
}
