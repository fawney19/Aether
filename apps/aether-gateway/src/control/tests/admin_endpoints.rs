use http::Uri;

use super::{classify_control_route, headers};

#[test]
fn classifies_admin_endpoint_health_summary_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/endpoints/health/summary"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("endpoints_health"));
    assert_eq!(decision.route_kind.as_deref(), Some("health_summary"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:endpoints_health")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_endpoint_key_health_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/endpoints/health/key/key-openai?api_format=openai:chat"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("endpoints_health"));
    assert_eq!(decision.route_kind.as_deref(), Some("key_health"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:endpoints_health")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_endpoint_recover_key_health_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/endpoints/health/keys/key-openai?api_format=openai:chat"
        .parse()
        .expect("uri should parse");
    let decision = classify_control_route(&http::Method::PATCH, &uri, &headers)
        .expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("endpoints_health"));
    assert_eq!(decision.route_kind.as_deref(), Some("recover_key_health"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:endpoints_health")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_endpoint_recover_all_keys_health_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/endpoints/health/keys"
        .parse()
        .expect("uri should parse");
    let decision = classify_control_route(&http::Method::PATCH, &uri, &headers)
        .expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("endpoints_health"));
    assert_eq!(
        decision.route_kind.as_deref(),
        Some("recover_all_keys_health")
    );
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:endpoints_health")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_endpoint_health_status_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/endpoints/health/status?lookback_hours=12"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("endpoints_health"));
    assert_eq!(decision.route_kind.as_deref(), Some("health_status"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:endpoints_health")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_endpoint_key_rpm_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/endpoints/rpm/key/key-1"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("endpoints_rpm"));
    assert_eq!(decision.route_kind.as_deref(), Some("key_rpm"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:endpoints_rpm")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_endpoint_reset_key_rpm_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/endpoints/rpm/key/key-1"
        .parse()
        .expect("uri should parse");
    let decision = classify_control_route(&http::Method::DELETE, &uri, &headers)
        .expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("endpoints_rpm"));
    assert_eq!(decision.route_kind.as_deref(), Some("reset_key_rpm"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:endpoints_rpm")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_list_provider_endpoints_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/endpoints/providers/provider-1/endpoints?skip=0&limit=50"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("endpoints_manage"));
    assert_eq!(
        decision.route_kind.as_deref(),
        Some("list_provider_endpoints")
    );
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:endpoints_manage")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_list_provider_keys_as_admin_proxy_route() {
    let headers = http::HeaderMap::new();
    let uri: Uri = "/api/admin/endpoints/providers/provider-openai/keys?skip=0&limit=20"
        .parse()
        .expect("uri should parse");
    let decision = classify_control_route(&http::Method::GET, &uri, &headers)
        .expect("decision should resolve");
    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("endpoints_manage"));
    assert_eq!(decision.route_kind.as_deref(), Some("list_provider_keys"));
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_keys_grouped_by_format_as_admin_proxy_route() {
    let headers = http::HeaderMap::new();
    let uri: Uri = "/api/admin/endpoints/keys/grouped-by-format"
        .parse()
        .expect("uri should parse");
    let decision = classify_control_route(&http::Method::GET, &uri, &headers)
        .expect("decision should resolve");
    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("endpoints_manage"));
    assert_eq!(
        decision.route_kind.as_deref(),
        Some("keys_grouped_by_format")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_reveal_key_as_admin_proxy_route() {
    let headers = http::HeaderMap::new();
    let uri: Uri = "/api/admin/endpoints/keys/key-openai/reveal"
        .parse()
        .expect("uri should parse");
    let decision = classify_control_route(&http::Method::GET, &uri, &headers)
        .expect("decision should resolve");
    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("endpoints_manage"));
    assert_eq!(decision.route_kind.as_deref(), Some("reveal_key"));
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_export_key_as_admin_proxy_route() {
    let headers = http::HeaderMap::new();
    let uri: Uri = "/api/admin/endpoints/keys/key-openai/export"
        .parse()
        .expect("uri should parse");
    let decision = classify_control_route(&http::Method::GET, &uri, &headers)
        .expect("decision should resolve");
    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("endpoints_manage"));
    assert_eq!(decision.route_kind.as_deref(), Some("export_key"));
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_update_key_as_admin_proxy_route() {
    let headers = http::HeaderMap::new();
    let uri: Uri = "/api/admin/endpoints/keys/key-openai"
        .parse()
        .expect("uri should parse");
    let decision = classify_control_route(&http::Method::PUT, &uri, &headers)
        .expect("decision should resolve");
    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("endpoints_manage"));
    assert_eq!(decision.route_kind.as_deref(), Some("update_key"));
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_delete_key_as_admin_proxy_route() {
    let headers = http::HeaderMap::new();
    let uri: Uri = "/api/admin/endpoints/keys/key-openai"
        .parse()
        .expect("uri should parse");
    let decision = classify_control_route(&http::Method::DELETE, &uri, &headers)
        .expect("decision should resolve");
    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("endpoints_manage"));
    assert_eq!(decision.route_kind.as_deref(), Some("delete_key"));
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_batch_delete_keys_as_admin_proxy_route() {
    let headers = http::HeaderMap::new();
    let uri: Uri = "/api/admin/endpoints/keys/batch-delete"
        .parse()
        .expect("uri should parse");
    let decision = classify_control_route(&http::Method::POST, &uri, &headers)
        .expect("decision should resolve");
    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("endpoints_manage"));
    assert_eq!(decision.route_kind.as_deref(), Some("batch_delete_keys"));
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_clear_oauth_invalid_as_admin_proxy_route() {
    let headers = http::HeaderMap::new();
    let uri: Uri = "/api/admin/endpoints/keys/key-openai/clear-oauth-invalid"
        .parse()
        .expect("uri should parse");
    let decision = classify_control_route(&http::Method::POST, &uri, &headers)
        .expect("decision should resolve");
    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("endpoints_manage"));
    assert_eq!(decision.route_kind.as_deref(), Some("clear_oauth_invalid"));
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_create_provider_key_as_admin_proxy_route() {
    let headers = http::HeaderMap::new();
    let uri: Uri = "/api/admin/endpoints/providers/provider-openai/keys"
        .parse()
        .expect("uri should parse");
    let decision = classify_control_route(&http::Method::POST, &uri, &headers)
        .expect("decision should resolve");
    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("endpoints_manage"));
    assert_eq!(decision.route_kind.as_deref(), Some("create_provider_key"));
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_get_endpoint_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/endpoints/endpoint-1"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("endpoints_manage"));
    assert_eq!(decision.route_kind.as_deref(), Some("get_endpoint"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:endpoints_manage")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_create_endpoint_as_admin_proxy_route() {
    let headers = http::HeaderMap::new();
    let uri: Uri = "/api/admin/endpoints/providers/provider-openai/endpoints"
        .parse()
        .expect("uri should parse");
    let decision = classify_control_route(&http::Method::POST, &uri, &headers)
        .expect("decision should resolve");
    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("endpoints_manage"));
    assert_eq!(decision.route_kind.as_deref(), Some("create_endpoint"));
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_update_endpoint_as_admin_proxy_route() {
    let headers = http::HeaderMap::new();
    let uri: Uri = "/api/admin/endpoints/endpoint-openai-chat"
        .parse()
        .expect("uri should parse");
    let decision = classify_control_route(&http::Method::PUT, &uri, &headers)
        .expect("decision should resolve");
    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("endpoints_manage"));
    assert_eq!(decision.route_kind.as_deref(), Some("update_endpoint"));
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_delete_endpoint_as_admin_proxy_route() {
    let headers = http::HeaderMap::new();
    let uri: Uri = "/api/admin/endpoints/endpoint-openai-chat"
        .parse()
        .expect("uri should parse");
    let decision = classify_control_route(&http::Method::DELETE, &uri, &headers)
        .expect("decision should resolve");
    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("endpoints_manage"));
    assert_eq!(decision.route_kind.as_deref(), Some("delete_endpoint"));
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_refresh_provider_quota_as_admin_proxy_route() {
    let headers = http::HeaderMap::new();
    let uri: Uri = "/api/admin/endpoints/providers/provider-codex/refresh-quota"
        .parse()
        .expect("uri should parse");
    let decision = classify_control_route(&http::Method::POST, &uri, &headers)
        .expect("decision should resolve");
    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("endpoints_manage"));
    assert_eq!(decision.route_kind.as_deref(), Some("refresh_quota"));
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_default_body_rules_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/endpoints/defaults/openai:responses/body-rules?provider_type=codex"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("endpoints_manage"));
    assert_eq!(decision.route_kind.as_deref(), Some("default_body_rules"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:endpoints_manage")
    );
    assert!(!decision.is_execution_runtime_candidate());
}
