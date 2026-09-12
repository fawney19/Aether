use http::Uri;

use crate::handlers::shared::local_proxy_route_requires_buffered_body;

use super::{classify_control_route, headers, GatewayPublicRequestContext};

#[test]
fn classifies_admin_users_list_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/users".parse().expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("users_manage"));
    assert_eq!(decision.route_kind.as_deref(), Some("list_users"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:users")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_users_create_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/users".parse().expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::POST, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("users_manage"));
    assert_eq!(decision.route_kind.as_deref(), Some("create_user"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:users")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_user_batch_routes_as_admin_proxy_route() {
    let headers = headers(&[]);

    let resolve_uri: Uri = "/api/admin/users/resolve-selection"
        .parse()
        .expect("uri should parse");
    let resolve = classify_control_route(&http::Method::POST, &resolve_uri, &headers)
        .expect("route should classify");
    assert_eq!(resolve.route_family.as_deref(), Some("users_manage"));
    assert_eq!(
        resolve.route_kind.as_deref(),
        Some("resolve_user_selection")
    );
    assert_eq!(
        resolve.auth_endpoint_signature.as_deref(),
        Some("admin:users")
    );

    let batch_uri: Uri = "/api/admin/users/batch-action"
        .parse()
        .expect("uri should parse");
    let batch = classify_control_route(&http::Method::POST, &batch_uri, &headers)
        .expect("route should classify");
    assert_eq!(batch.route_family.as_deref(), Some("users_manage"));
    assert_eq!(batch.route_kind.as_deref(), Some("batch_action_users"));
    assert_eq!(
        batch.auth_endpoint_signature.as_deref(),
        Some("admin:users")
    );
}

#[test]
fn classifies_admin_user_billing_routes_as_admin_proxy_route() {
    let headers = headers(&[]);

    let entitlements_uri: Uri = "/api/admin/users/user-1/billing/entitlements"
        .parse()
        .expect("uri should parse");
    let entitlements = classify_control_route(&http::Method::GET, &entitlements_uri, &headers)
        .expect("route should classify");
    assert_eq!(entitlements.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(entitlements.route_family.as_deref(), Some("users_manage"));
    assert_eq!(
        entitlements.route_kind.as_deref(),
        Some("list_user_billing_entitlements")
    );
    assert_eq!(
        entitlements.auth_endpoint_signature.as_deref(),
        Some("admin:users")
    );

    let grant_uri: Uri = "/api/admin/users/user-1/billing/grant-plan"
        .parse()
        .expect("uri should parse");
    let grant = classify_control_route(&http::Method::POST, &grant_uri, &headers)
        .expect("route should classify");
    assert_eq!(grant.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(grant.route_family.as_deref(), Some("users_manage"));
    assert_eq!(grant.route_kind.as_deref(), Some("grant_user_billing_plan"));
    assert_eq!(
        grant.auth_endpoint_signature.as_deref(),
        Some("admin:users")
    );

    let revoke_uri: Uri = "/api/admin/users/user-1/billing/entitlements/entitlement-1"
        .parse()
        .expect("uri should parse");
    let revoke = classify_control_route(&http::Method::DELETE, &revoke_uri, &headers)
        .expect("route should classify");
    assert_eq!(revoke.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(revoke.route_family.as_deref(), Some("users_manage"));
    assert_eq!(
        revoke.route_kind.as_deref(),
        Some("revoke_user_billing_entitlement")
    );
    assert_eq!(
        revoke.auth_endpoint_signature.as_deref(),
        Some("admin:users")
    );

    let context = GatewayPublicRequestContext::from_request_parts(
        "trace-user-billing-grant",
        &http::Method::POST,
        &grant_uri,
        &headers,
        Some(grant),
    );
    assert!(local_proxy_route_requires_buffered_body(&context));
}

#[test]
fn classifies_admin_user_group_routes_as_admin_proxy_route() {
    let headers = headers(&[]);

    let list_uri: Uri = "/api/admin/user-groups".parse().expect("uri should parse");
    let list = classify_control_route(&http::Method::GET, &list_uri, &headers)
        .expect("route should classify");
    assert_eq!(list.route_family.as_deref(), Some("users_manage"));
    assert_eq!(list.route_kind.as_deref(), Some("list_user_groups"));

    let create_uri: Uri = "/api/admin/user-groups".parse().expect("uri should parse");
    let create = classify_control_route(&http::Method::POST, &create_uri, &headers)
        .expect("route should classify");
    assert_eq!(create.route_family.as_deref(), Some("users_manage"));
    assert_eq!(create.route_kind.as_deref(), Some("create_user_group"));

    let update_uri: Uri = "/api/admin/user-groups/group-1"
        .parse()
        .expect("uri should parse");
    let update = classify_control_route(&http::Method::PUT, &update_uri, &headers)
        .expect("route should classify");
    assert_eq!(update.route_family.as_deref(), Some("users_manage"));
    assert_eq!(update.route_kind.as_deref(), Some("update_user_group"));

    let members_uri: Uri = "/api/admin/user-groups/group-1/members"
        .parse()
        .expect("uri should parse");
    let members = classify_control_route(&http::Method::PUT, &members_uri, &headers)
        .expect("route should classify");
    assert_eq!(members.route_family.as_deref(), Some("users_manage"));
    assert_eq!(
        members.route_kind.as_deref(),
        Some("replace_user_group_members")
    );

    let default_uri: Uri = "/api/admin/user-groups/default"
        .parse()
        .expect("uri should parse");
    let default = classify_control_route(&http::Method::PUT, &default_uri, &headers)
        .expect("route should classify");
    assert_eq!(default.route_family.as_deref(), Some("users_manage"));
    assert_eq!(
        default.route_kind.as_deref(),
        Some("set_default_user_group")
    );
    assert_eq!(
        default.auth_endpoint_signature.as_deref(),
        Some("admin:users")
    );
}

#[test]
fn admin_user_group_write_routes_buffer_request_body() {
    let headers = headers(&[]);
    let routes = [
        (http::Method::POST, "/api/admin/user-groups"),
        (http::Method::PUT, "/api/admin/user-groups/group-1"),
        (http::Method::PUT, "/api/admin/user-groups/group-1/members"),
        (http::Method::PUT, "/api/admin/user-groups/default"),
    ];

    for (method, path) in routes {
        let uri: Uri = path.parse().expect("uri should parse");
        let decision =
            classify_control_route(&method, &uri, &headers).expect("route should classify");
        let context = GatewayPublicRequestContext::from_request_parts(
            "trace-user-group-write",
            &method,
            &uri,
            &headers,
            Some(decision),
        );

        assert!(
            local_proxy_route_requires_buffered_body(&context),
            "{method} {path} should buffer request body"
        );
    }
}

#[test]
fn classifies_admin_user_detail_routes_as_admin_proxy_route() {
    let headers = headers(&[]);

    let get_uri: Uri = "/api/admin/users/user-1".parse().expect("uri should parse");
    let get = classify_control_route(&http::Method::GET, &get_uri, &headers)
        .expect("route should classify");
    assert_eq!(get.route_family.as_deref(), Some("users_manage"));
    assert_eq!(get.route_kind.as_deref(), Some("get_user"));

    let put_uri: Uri = "/api/admin/users/user-1".parse().expect("uri should parse");
    let put = classify_control_route(&http::Method::PUT, &put_uri, &headers)
        .expect("route should classify");
    assert_eq!(put.route_family.as_deref(), Some("users_manage"));
    assert_eq!(put.route_kind.as_deref(), Some("update_user"));

    let delete_uri: Uri = "/api/admin/users/user-1".parse().expect("uri should parse");
    let delete = classify_control_route(&http::Method::DELETE, &delete_uri, &headers)
        .expect("route should classify");
    assert_eq!(delete.route_family.as_deref(), Some("users_manage"));
    assert_eq!(delete.route_kind.as_deref(), Some("delete_user"));
    assert_eq!(
        delete.auth_endpoint_signature.as_deref(),
        Some("admin:users")
    );
}

#[test]
fn classifies_admin_user_session_routes_as_admin_proxy_route() {
    let headers = headers(&[]);

    let list_uri: Uri = "/api/admin/users/user-1/sessions"
        .parse()
        .expect("uri should parse");
    let list = classify_control_route(&http::Method::GET, &list_uri, &headers)
        .expect("route should classify");
    assert_eq!(list.route_family.as_deref(), Some("users_manage"));
    assert_eq!(list.route_kind.as_deref(), Some("list_user_sessions"));

    let delete_all_uri: Uri = "/api/admin/users/user-1/sessions"
        .parse()
        .expect("uri should parse");
    let delete_all = classify_control_route(&http::Method::DELETE, &delete_all_uri, &headers)
        .expect("route should classify");
    assert_eq!(delete_all.route_family.as_deref(), Some("users_manage"));
    assert_eq!(
        delete_all.route_kind.as_deref(),
        Some("delete_user_sessions")
    );

    let delete_one_uri: Uri = "/api/admin/users/user-1/sessions/session-1"
        .parse()
        .expect("uri should parse");
    let delete_one = classify_control_route(&http::Method::DELETE, &delete_one_uri, &headers)
        .expect("route should classify");
    assert_eq!(delete_one.route_family.as_deref(), Some("users_manage"));
    assert_eq!(
        delete_one.route_kind.as_deref(),
        Some("delete_user_session")
    );
    assert_eq!(
        delete_one.auth_endpoint_signature.as_deref(),
        Some("admin:users")
    );
}

#[test]
fn classifies_admin_user_api_key_routes_as_admin_proxy_route() {
    let headers = headers(&[]);

    let list_uri: Uri = "/api/admin/users/user-1/api-keys"
        .parse()
        .expect("uri should parse");
    let list = classify_control_route(&http::Method::GET, &list_uri, &headers)
        .expect("route should classify");
    assert_eq!(list.route_family.as_deref(), Some("users_manage"));
    assert_eq!(list.route_kind.as_deref(), Some("list_user_api_keys"));

    let create_uri: Uri = "/api/admin/users/user-1/api-keys"
        .parse()
        .expect("uri should parse");
    let create = classify_control_route(&http::Method::POST, &create_uri, &headers)
        .expect("route should classify");
    assert_eq!(create.route_family.as_deref(), Some("users_manage"));
    assert_eq!(create.route_kind.as_deref(), Some("create_user_api_key"));

    let delete_uri: Uri = "/api/admin/users/user-1/api-keys/key-1"
        .parse()
        .expect("uri should parse");
    let delete = classify_control_route(&http::Method::DELETE, &delete_uri, &headers)
        .expect("route should classify");
    assert_eq!(delete.route_family.as_deref(), Some("users_manage"));
    assert_eq!(delete.route_kind.as_deref(), Some("delete_user_api_key"));

    let update_uri: Uri = "/api/admin/users/user-1/api-keys/key-1"
        .parse()
        .expect("uri should parse");
    let update = classify_control_route(&http::Method::PUT, &update_uri, &headers)
        .expect("route should classify");
    assert_eq!(update.route_family.as_deref(), Some("users_manage"));
    assert_eq!(update.route_kind.as_deref(), Some("update_user_api_key"));

    let lock_uri: Uri = "/api/admin/users/user-1/api-keys/key-1/lock"
        .parse()
        .expect("uri should parse");
    let lock = classify_control_route(&http::Method::PATCH, &lock_uri, &headers)
        .expect("route should classify");
    assert_eq!(lock.route_family.as_deref(), Some("users_manage"));
    assert_eq!(lock.route_kind.as_deref(), Some("lock_user_api_key"));

    let full_key_uri: Uri = "/api/admin/users/user-1/api-keys/key-1/full-key"
        .parse()
        .expect("uri should parse");
    let full_key = classify_control_route(&http::Method::GET, &full_key_uri, &headers)
        .expect("route should classify");
    assert_eq!(full_key.route_family.as_deref(), Some("users_manage"));
    assert_eq!(full_key.route_kind.as_deref(), Some("reveal_user_api_key"));
    assert_eq!(
        full_key.auth_endpoint_signature.as_deref(),
        Some("admin:users")
    );
}

#[test]
fn classifies_admin_user_api_key_mutation_routes_with_admin_users_signature() {
    let headers = headers(&[]);

    let create_uri: Uri = "/api/admin/users/user-1/api-keys"
        .parse()
        .expect("uri should parse");
    let create = classify_control_route(&http::Method::POST, &create_uri, &headers)
        .expect("route should classify");
    assert_eq!(create.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(
        create.auth_endpoint_signature.as_deref(),
        Some("admin:users")
    );

    let lock_uri: Uri = "/api/admin/users/user-1/api-keys/key-1/lock"
        .parse()
        .expect("uri should parse");
    let lock = classify_control_route(&http::Method::PATCH, &lock_uri, &headers)
        .expect("route should classify");
    assert_eq!(lock.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(lock.auth_endpoint_signature.as_deref(), Some("admin:users"));
}
