use http::Uri;

use super::{classify_control_route, headers};

#[test]
fn classifies_admin_payments_list_orders_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/payments/orders"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("payments_manage"));
    assert_eq!(decision.route_kind.as_deref(), Some("list_orders"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:payments")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_payments_get_order_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/payments/orders/order-1"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("payments_manage"));
    assert_eq!(decision.route_kind.as_deref(), Some("get_order"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:payments")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_payments_trailing_slash_routes_as_admin_proxy_route() {
    let headers = headers(&[]);

    let detail_uri: Uri = "/api/admin/payments/orders/order-1/"
        .parse()
        .expect("uri should parse");
    let detail = classify_control_route(&http::Method::GET, &detail_uri, &headers)
        .expect("detail route should classify");
    assert_eq!(detail.route_family.as_deref(), Some("payments_manage"));
    assert_eq!(detail.route_kind.as_deref(), Some("get_order"));

    let credit_uri: Uri = "/api/admin/payments/orders/order-1/credit/"
        .parse()
        .expect("uri should parse");
    let credit = classify_control_route(&http::Method::POST, &credit_uri, &headers)
        .expect("credit route should classify");
    assert_eq!(credit.route_family.as_deref(), Some("payments_manage"));
    assert_eq!(credit.route_kind.as_deref(), Some("credit_order"));
}

#[test]
fn classifies_admin_payments_expire_order_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/payments/orders/order-1/expire"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::POST, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("payments_manage"));
    assert_eq!(decision.route_kind.as_deref(), Some("expire_order"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:payments")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_payments_credit_order_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/payments/orders/order-1/credit"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::POST, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("payments_manage"));
    assert_eq!(decision.route_kind.as_deref(), Some("credit_order"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:payments")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_payments_fail_order_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/payments/orders/order-1/fail"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::POST, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("payments_manage"));
    assert_eq!(decision.route_kind.as_deref(), Some("fail_order"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:payments")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_payments_callbacks_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/payments/callbacks"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("payments_manage"));
    assert_eq!(decision.route_kind.as_deref(), Some("list_callbacks"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:payments")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_payments_redeem_code_routes_as_admin_proxy_route() {
    let headers = headers(&[]);

    let list_batches_uri: Uri = "/api/admin/payments/redeem-codes/batches"
        .parse()
        .expect("uri should parse");
    let list_batches = classify_control_route(&http::Method::GET, &list_batches_uri, &headers)
        .expect("route should classify");
    assert_eq!(
        list_batches.route_family.as_deref(),
        Some("payments_manage")
    );
    assert_eq!(
        list_batches.route_kind.as_deref(),
        Some("list_redeem_code_batches")
    );

    let create_batch_uri: Uri = "/api/admin/payments/redeem-codes/batches"
        .parse()
        .expect("uri should parse");
    let create_batch = classify_control_route(&http::Method::POST, &create_batch_uri, &headers)
        .expect("route should classify");
    assert_eq!(
        create_batch.route_family.as_deref(),
        Some("payments_manage")
    );
    assert_eq!(
        create_batch.route_kind.as_deref(),
        Some("create_redeem_code_batch")
    );

    let list_codes_uri: Uri = "/api/admin/payments/redeem-codes/batches/batch-1/codes"
        .parse()
        .expect("uri should parse");
    let list_codes = classify_control_route(&http::Method::GET, &list_codes_uri, &headers)
        .expect("route should classify");
    assert_eq!(list_codes.route_family.as_deref(), Some("payments_manage"));
    assert_eq!(list_codes.route_kind.as_deref(), Some("list_redeem_codes"));

    let disable_code_uri: Uri = "/api/admin/payments/redeem-codes/codes/code-1/disable"
        .parse()
        .expect("uri should parse");
    let disable_code = classify_control_route(&http::Method::POST, &disable_code_uri, &headers)
        .expect("route should classify");
    assert_eq!(
        disable_code.route_family.as_deref(),
        Some("payments_manage")
    );
    assert_eq!(
        disable_code.route_kind.as_deref(),
        Some("disable_redeem_code")
    );

    let delete_batch_uri: Uri = "/api/admin/payments/redeem-codes/batches/batch-1/delete"
        .parse()
        .expect("uri should parse");
    let delete_batch = classify_control_route(&http::Method::POST, &delete_batch_uri, &headers)
        .expect("route should classify");
    assert_eq!(
        delete_batch.route_family.as_deref(),
        Some("payments_manage")
    );
    assert_eq!(
        delete_batch.route_kind.as_deref(),
        Some("delete_redeem_code_batch")
    );
}
