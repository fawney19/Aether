use http::Uri;

use super::{classify_control_route, headers};

#[test]
fn classifies_models_list_as_public_support_route() {
    let headers = headers(&[("authorization", "Bearer sk-test")]);
    let uri: Uri = "/v1/models".parse().expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("public_support"));
    assert_eq!(decision.route_family.as_deref(), Some("models"));
    assert_eq!(decision.route_kind.as_deref(), Some("list"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("openai:chat")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_v1beta_models_as_gemini_public_support_route() {
    let headers = headers(&[]);
    let uri: Uri = "/v1beta/models?pageSize=10"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("public_support"));
    assert_eq!(decision.route_family.as_deref(), Some("models"));
    assert_eq!(decision.route_kind.as_deref(), Some("list"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("gemini:chat")
    );
}

#[test]
fn classifies_public_catalog_site_info_as_public_support_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/public/site-info".parse().expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("public_support"));
    assert_eq!(decision.route_family.as_deref(), Some("public_catalog"));
    assert_eq!(decision.route_kind.as_deref(), Some("site_info"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("public:catalog")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_public_announcement_list_as_public_support_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/announcements?limit=20"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("public_support"));
    assert_eq!(decision.route_family.as_deref(), Some("announcements"));
    assert_eq!(decision.route_kind.as_deref(), Some("list"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("public:announcements")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_announcement_create_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/announcements".parse().expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::POST, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(
        decision.route_family.as_deref(),
        Some("announcements_manage")
    );
    assert_eq!(decision.route_kind.as_deref(), Some("create_announcement"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:announcements")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_announcement_update_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/announcements/announcement-1"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::PUT, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(
        decision.route_family.as_deref(),
        Some("announcements_manage")
    );
    assert_eq!(decision.route_kind.as_deref(), Some("update_announcement"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:announcements")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_admin_announcement_delete_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/announcements/announcement-1"
        .parse()
        .expect("uri should parse");
    let decision = classify_control_route(&http::Method::DELETE, &uri, &headers)
        .expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(
        decision.route_family.as_deref(),
        Some("announcements_manage")
    );
    assert_eq!(decision.route_kind.as_deref(), Some("delete_announcement"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("admin:announcements")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_public_active_announcements_as_public_support_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/announcements/active"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("public_support"));
    assert_eq!(decision.route_family.as_deref(), Some("announcements"));
    assert_eq!(decision.route_kind.as_deref(), Some("active"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("public:announcements")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_public_announcement_detail_as_public_support_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/announcements/announcement-1"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("public_support"));
    assert_eq!(decision.route_family.as_deref(), Some("announcements"));
    assert_eq!(decision.route_kind.as_deref(), Some("detail"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("public:announcements")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_dashboard_stats_as_public_support_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/dashboard/stats".parse().expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("public_support"));
    assert_eq!(decision.route_family.as_deref(), Some("dashboard"));
    assert_eq!(decision.route_kind.as_deref(), Some("stats"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("user:dashboard")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_user_monitoring_audit_logs_as_public_support_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/monitoring/my-audit-logs"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("public_support"));
    assert_eq!(decision.route_family.as_deref(), Some("monitoring_user"));
    assert_eq!(decision.route_kind.as_deref(), Some("audit_logs"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("user:monitoring")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_wallet_redeem_as_public_support_route() {
    let headers = headers(&[("authorization", "Bearer sk-test")]);
    let uri: Uri = "/api/wallet/redeem".parse().expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::POST, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("public_support"));
    assert_eq!(decision.route_family.as_deref(), Some("wallet"));
    assert_eq!(decision.route_kind.as_deref(), Some("redeem"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("user:wallet")
    );
}

#[test]
fn classifies_announcement_unread_count_as_public_support_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/announcements/users/me/unread-count"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("public_support"));
    assert_eq!(decision.route_family.as_deref(), Some("announcement_user"));
    assert_eq!(decision.route_kind.as_deref(), Some("unread_count"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("user:announcements")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_announcement_read_status_as_public_support_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/announcements/announcement-1/read-status"
        .parse()
        .expect("uri should parse");
    let decision = classify_control_route(&http::Method::PATCH, &uri, &headers)
        .expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("public_support"));
    assert_eq!(decision.route_family.as_deref(), Some("announcement_user"));
    assert_eq!(decision.route_kind.as_deref(), Some("read_status"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("user:announcements")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_announcement_read_all_as_public_support_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/announcements/read-all"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::POST, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("public_support"));
    assert_eq!(decision.route_family.as_deref(), Some("announcement_user"));
    assert_eq!(decision.route_kind.as_deref(), Some("read_all"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("user:announcements")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_wallet_routes_as_public_support_route() {
    let headers = headers(&[]);
    for (method, uri, route_kind) in [
        (http::Method::GET, "/api/wallet/balance", "balance"),
        (
            http::Method::GET,
            "/api/wallet/transactions?limit=20",
            "transactions",
        ),
        (http::Method::GET, "/api/wallet/flow?limit=20", "flow"),
        (http::Method::GET, "/api/wallet/today-cost", "today_cost"),
        (
            http::Method::GET,
            "/api/wallet/recharge?limit=20",
            "list_recharge_orders",
        ),
        (
            http::Method::POST,
            "/api/wallet/recharge",
            "create_recharge_order",
        ),
        (
            http::Method::GET,
            "/api/wallet/recharge/order-1",
            "recharge_detail",
        ),
        (
            http::Method::GET,
            "/api/wallet/refunds?limit=20",
            "list_refunds",
        ),
        (http::Method::POST, "/api/wallet/refunds", "create_refund"),
        (
            http::Method::GET,
            "/api/wallet/refunds/refund-1",
            "refund_detail",
        ),
    ] {
        let uri: Uri = uri.parse().expect("uri should parse");
        let decision =
            classify_control_route(&method, &uri, &headers).expect("route should classify");

        assert_eq!(decision.route_class.as_deref(), Some("public_support"));
        assert_eq!(decision.route_family.as_deref(), Some("wallet"));
        assert_eq!(decision.route_kind.as_deref(), Some(route_kind));
        assert_eq!(
            decision.auth_endpoint_signature.as_deref(),
            Some("user:wallet")
        );
        assert!(!decision.is_execution_runtime_candidate());
    }
}

#[test]
fn classifies_users_me_routes_as_public_support_route() {
    let headers = headers(&[]);
    for (method, uri, route_kind) in [
        (http::Method::GET, "/api/users/me", "detail"),
        (http::Method::PUT, "/api/users/me", "update_detail"),
        (http::Method::PATCH, "/api/users/me/password", "password"),
        (http::Method::GET, "/api/users/me/sessions", "sessions"),
        (
            http::Method::DELETE,
            "/api/users/me/sessions/others",
            "sessions_others_delete",
        ),
        (
            http::Method::PATCH,
            "/api/users/me/sessions/session-1",
            "session_update",
        ),
        (
            http::Method::GET,
            "/api/users/me/api-keys/key-1",
            "api_key_detail",
        ),
        (
            http::Method::POST,
            "/api/users/me/api-keys",
            "api_keys_create",
        ),
        (
            http::Method::PUT,
            "/api/users/me/api-keys/key-1",
            "api_key_update",
        ),
        (
            http::Method::PATCH,
            "/api/users/me/api-keys/key-1",
            "api_key_patch",
        ),
        (
            http::Method::DELETE,
            "/api/users/me/api-keys/key-1",
            "api_key_delete",
        ),
        (
            http::Method::PUT,
            "/api/users/me/api-keys/key-1/providers",
            "api_key_providers_update",
        ),
        (
            http::Method::PUT,
            "/api/users/me/api-keys/key-1/capabilities",
            "api_key_capabilities_update",
        ),
        (
            http::Method::GET,
            "/api/users/me/available-models",
            "available_models",
        ),
        (
            http::Method::PUT,
            "/api/users/me/model-capabilities",
            "model_capabilities_update",
        ),
        (
            http::Method::GET,
            "/api/me/management-tokens",
            "management_tokens_list",
        ),
        (
            http::Method::POST,
            "/api/me/management-tokens",
            "management_tokens_create",
        ),
        (
            http::Method::GET,
            "/api/me/management-tokens/token-1",
            "management_token_detail",
        ),
        (
            http::Method::PUT,
            "/api/me/management-tokens/token-1",
            "management_token_update",
        ),
        (
            http::Method::DELETE,
            "/api/me/management-tokens/token-1",
            "management_token_delete",
        ),
        (
            http::Method::PATCH,
            "/api/me/management-tokens/token-1/status",
            "management_token_toggle",
        ),
        (
            http::Method::POST,
            "/api/me/management-tokens/token-1/regenerate",
            "management_token_regenerate",
        ),
    ] {
        let uri: Uri = uri.parse().expect("uri should parse");
        let decision =
            classify_control_route(&method, &uri, &headers).expect("route should classify");

        assert_eq!(decision.route_class.as_deref(), Some("public_support"));
        assert_eq!(decision.route_family.as_deref(), Some("users_me"));
        assert_eq!(decision.route_kind.as_deref(), Some(route_kind));
        assert_eq!(
            decision.auth_endpoint_signature.as_deref(),
            Some("user:self")
        );
        assert!(!decision.is_execution_runtime_candidate());
    }
}

#[test]
fn classifies_payment_callback_as_public_support_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/payment/callback/alipay"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::POST, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("public_support"));
    assert_eq!(decision.route_family.as_deref(), Some("payment_callback"));
    assert_eq!(decision.route_kind.as_deref(), Some("callback"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("public:payment")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_public_catalog_providers_as_public_support_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/public/providers?limit=20"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("public_support"));
    assert_eq!(decision.route_family.as_deref(), Some("public_catalog"));
    assert_eq!(decision.route_kind.as_deref(), Some("providers"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("public:catalog")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_public_catalog_models_as_public_support_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/public/models?provider_id=provider-openai"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("public_support"));
    assert_eq!(decision.route_family.as_deref(), Some("public_catalog"));
    assert_eq!(decision.route_kind.as_deref(), Some("models"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("public:catalog")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_public_catalog_search_models_as_public_support_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/public/search/models?q=gpt"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("public_support"));
    assert_eq!(decision.route_family.as_deref(), Some("public_catalog"));
    assert_eq!(decision.route_kind.as_deref(), Some("search_models"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("public:catalog")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_public_catalog_stats_as_public_support_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/public/stats".parse().expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("public_support"));
    assert_eq!(decision.route_family.as_deref(), Some("public_catalog"));
    assert_eq!(decision.route_kind.as_deref(), Some("stats"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("public:catalog")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_public_catalog_global_models_as_public_support_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/public/global-models?limit=10"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("public_support"));
    assert_eq!(decision.route_family.as_deref(), Some("public_catalog"));
    assert_eq!(decision.route_kind.as_deref(), Some("global_models"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("public:catalog")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_public_catalog_health_api_formats_as_public_support_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/public/health/api-formats?lookback_hours=12"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("public_support"));
    assert_eq!(decision.route_family.as_deref(), Some("public_catalog"));
    assert_eq!(decision.route_kind.as_deref(), Some("health_api_formats"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("public:catalog")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_auth_registration_settings_as_public_support_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/auth/registration-settings"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("public_support"));
    assert_eq!(decision.route_family.as_deref(), Some("auth_public"));
    assert_eq!(
        decision.route_kind.as_deref(),
        Some("registration_settings")
    );
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("public:auth")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_auth_settings_as_public_support_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/auth/settings".parse().expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("public_support"));
    assert_eq!(decision.route_family.as_deref(), Some("auth_public"));
    assert_eq!(decision.route_kind.as_deref(), Some("settings"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("public:auth")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_auth_routes_as_public_support_route() {
    for (method, path, route_kind) in [
        (http::Method::POST, "/api/auth/login", "login"),
        (http::Method::POST, "/api/auth/refresh", "refresh"),
        (http::Method::POST, "/api/auth/register", "register"),
        (http::Method::GET, "/api/auth/me", "me"),
        (http::Method::POST, "/api/auth/logout", "logout"),
        (
            http::Method::POST,
            "/api/auth/send-verification-code",
            "send_verification_code",
        ),
        (http::Method::POST, "/api/auth/verify-email", "verify_email"),
        (
            http::Method::POST,
            "/api/auth/verification-status",
            "verification_status",
        ),
    ] {
        let headers = headers(&[]);
        let uri: Uri = path.parse().expect("uri should parse");
        let decision =
            classify_control_route(&method, &uri, &headers).expect("route should classify");

        assert_eq!(decision.route_class.as_deref(), Some("public_support"));
        assert_eq!(decision.route_family.as_deref(), Some("auth"));
        assert_eq!(decision.route_kind.as_deref(), Some(route_kind));
        assert_eq!(
            decision.auth_endpoint_signature.as_deref(),
            Some("user:auth")
        );
        assert!(!decision.is_execution_runtime_candidate());
    }
}

#[test]
fn does_not_classify_oauth_public_providers_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/oauth/providers".parse().expect("uri should parse");
    let decision = classify_control_route(&http::Method::GET, &uri, &headers);

    assert!(decision.is_none());
}

#[test]
fn does_not_classify_oauth_public_authorize_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/oauth/linuxdo/authorize?client_device_id=device-1"
        .parse()
        .expect("uri should parse");
    let decision = classify_control_route(&http::Method::GET, &uri, &headers);

    assert!(decision.is_none());
}

#[test]
fn does_not_classify_oauth_user_bindable_providers_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/user/oauth/bindable-providers"
        .parse()
        .expect("uri should parse");
    let decision = classify_control_route(&http::Method::GET, &uri, &headers);

    assert!(decision.is_none());
}

#[test]
fn does_not_classify_oauth_user_bind_token_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/user/oauth/linuxdo/bind-token"
        .parse()
        .expect("uri should parse");
    let decision = classify_control_route(&http::Method::POST, &uri, &headers);

    assert!(decision.is_none());
}

#[test]
fn classifies_capabilities_list_as_public_support_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/capabilities".parse().expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("public_support"));
    assert_eq!(decision.route_family.as_deref(), Some("capabilities"));
    assert_eq!(decision.route_kind.as_deref(), Some("list"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("public:capabilities")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_capabilities_user_configurable_as_public_support_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/capabilities/user-configurable"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("public_support"));
    assert_eq!(decision.route_family.as_deref(), Some("capabilities"));
    assert_eq!(decision.route_kind.as_deref(), Some("user_configurable"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("public:capabilities")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_capabilities_model_as_public_support_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/capabilities/model/gpt-5"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("public_support"));
    assert_eq!(decision.route_family.as_deref(), Some("capabilities"));
    assert_eq!(decision.route_kind.as_deref(), Some("model"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("public:capabilities")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_modules_auth_status_as_public_support_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/modules/auth-status"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("public_support"));
    assert_eq!(decision.route_family.as_deref(), Some("modules"));
    assert_eq!(decision.route_kind.as_deref(), Some("auth_status"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("public:modules")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_system_catalog_provider_detail_as_public_support_route() {
    let headers = headers(&[]);
    let uri: Uri = "/v1/providers/provider-openai"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("public_support"));
    assert_eq!(decision.route_family.as_deref(), Some("system_catalog"));
    assert_eq!(decision.route_kind.as_deref(), Some("provider_detail"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("public:system_catalog")
    );
    assert!(!decision.is_execution_runtime_candidate());
}
