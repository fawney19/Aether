use super::*;

#[test]
fn admin_shared_does_not_own_provider_support() {
    let admin_shared = read_workspace_file("apps/aether-gateway/src/handlers/admin/shared/mod.rs");
    assert!(
        !admin_shared.contains("mod support;"),
        "handlers/admin/shared/mod.rs should not keep provider support module"
    );
    assert!(
        !workspace_file_exists("apps/aether-gateway/src/handlers/admin/shared/support.rs"),
        "handlers/admin/shared/support.rs should be removed after provider support extraction"
    );

    let provider_support =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/shared/support.rs");
    for pattern in [
        "pub(crate) struct AdminProviderPoolConfig",
        "pub(crate) struct AdminProviderPoolRuntimeState",
        "pub(crate) const ADMIN_PROVIDER_POOL_SCAN_BATCH",
        "pub(crate) const ADMIN_PROVIDER_OAUTH_DATA_UNAVAILABLE_DETAIL",
    ] {
        assert!(
            provider_support.contains(pattern),
            "provider/shared/support.rs should own {pattern}"
        );
    }

    let admin_shared_paths =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/shared/paths.rs");
    for pattern in [
        "pub(crate) fn admin_provider_id_for_manage_path",
        "pub(crate) fn admin_provider_oauth_start_key_id",
        "pub(crate) fn admin_provider_ops_architecture_id_from_path",
    ] {
        assert!(
            !admin_shared_paths.contains(pattern),
            "handlers/admin/shared/paths.rs should not own {pattern}"
        );
    }

    let provider_paths =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/shared/paths.rs");
    for pattern in [
        "pub(crate) fn admin_provider_id_for_manage_path",
        "pub(crate) fn admin_provider_oauth_start_key_id",
        "pub(crate) fn admin_provider_ops_architecture_id_from_path",
    ] {
        assert!(
            provider_paths.contains(pattern),
            "provider/shared/paths.rs should own {pattern}"
        );
    }

    let provider_shared_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/shared/mod.rs");
    for pattern in [
        "pub(crate) mod paths;",
        "pub(crate) mod payloads;",
        "pub(crate) mod support;",
    ] {
        assert!(
            provider_shared_mod.contains(pattern),
            "handlers/admin/provider/shared/mod.rs should expose explicit provider shared submodule {pattern}"
        );
    }
    for forbidden in [
        "pub(crate) use self::paths::*;",
        "pub(crate) use self::payloads::*;",
        "pub(crate) use self::support::*;",
    ] {
        assert!(
            !provider_shared_mod.contains(forbidden),
            "handlers/admin/provider/shared/mod.rs should not remain a wildcard re-export hub for {forbidden}"
        );
    }

    let admin_shared_payloads =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/shared/payloads.rs");
    for pattern in [
        "pub(crate) struct AdminProviderCreateRequest",
        "pub(crate) struct AdminProviderEndpointCreateRequest",
        "pub(crate) struct AdminProviderModelCreateRequest",
    ] {
        assert!(
            !admin_shared_payloads.contains(pattern),
            "handlers/admin/shared/payloads.rs should not own {pattern}"
        );
    }

    let provider_payloads =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/shared/payloads.rs");
    for pattern in [
        "pub(crate) struct AdminProviderCreateRequest",
        "pub(crate) struct AdminProviderModelCreateRequest",
    ] {
        assert!(
            provider_payloads.contains(pattern),
            "provider/shared/payloads.rs should own {pattern}"
        );
    }
    for pattern in [
        "AdminProviderEndpointCreateRequest",
        "AdminProviderEndpointUpdateRequest",
    ] {
        assert!(
            !provider_payloads.contains(pattern),
            "provider/shared/payloads.rs should not retain endpoint CRUD payload owner {pattern}"
        );
    }

    let provider_endpoints_payloads = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/endpoints_admin/payloads.rs",
    );
    for pattern in [
        "struct AdminProviderEndpointCreateRequest",
        "struct AdminProviderEndpointUpdateRequest",
    ] {
        assert!(
            provider_endpoints_payloads.contains(pattern),
            "provider/endpoints_admin/payloads.rs should own {pattern}"
        );
    }
}

#[test]
fn admin_shared_does_not_own_model_global_routes_or_payloads() {
    let admin_shared_paths =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/shared/paths.rs");
    for pattern in [
        "pub(crate) fn is_admin_global_models_root",
        "pub(crate) fn admin_global_model_id_from_path",
        "pub(crate) fn admin_global_model_routing_id",
    ] {
        assert!(
            !admin_shared_paths.contains(pattern),
            "handlers/admin/shared/paths.rs should not own {pattern}"
        );
    }

    let model_shared_paths =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/model/shared/paths.rs");
    for pattern in [
        "pub(crate) fn is_admin_global_models_root",
        "pub(crate) fn admin_global_model_id_from_path",
        "pub(crate) fn admin_global_model_routing_id",
    ] {
        assert!(
            model_shared_paths.contains(pattern),
            "model/shared/paths.rs should own {pattern}"
        );
    }

    let admin_shared_payloads =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/shared/payloads.rs");
    for pattern in [
        "pub(crate) struct AdminGlobalModelCreateRequest",
        "pub(crate) struct AdminGlobalModelUpdateRequest",
        "pub(crate) struct AdminBatchAssignToProvidersRequest",
    ] {
        assert!(
            !admin_shared_payloads.contains(pattern),
            "handlers/admin/shared/payloads.rs should not own {pattern}"
        );
    }

    let model_shared_payloads =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/model/shared/payloads.rs");
    for pattern in [
        "pub(crate) struct AdminGlobalModelCreateRequest",
        "pub(crate) struct AdminGlobalModelUpdateRequest",
        "pub(crate) struct AdminBatchAssignToProvidersRequest",
    ] {
        assert!(
            model_shared_payloads.contains(pattern),
            "model/shared/payloads.rs should own {pattern}"
        );
    }
}

#[test]
fn admin_shared_does_not_own_system_core_routes_or_payloads() {
    let admin_shared_paths =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/shared/paths.rs");
    for pattern in [
        "pub(crate) fn is_admin_management_tokens_root",
        "pub(crate) fn is_admin_system_configs_root",
        "pub(crate) fn admin_oauth_provider_type_from_path",
    ] {
        assert!(
            !admin_shared_paths.contains(pattern),
            "handlers/admin/shared/paths.rs should not own {pattern}"
        );
    }

    let system_shared_paths =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/system/shared/paths.rs");
    for pattern in [
        "pub(crate) fn is_admin_management_tokens_root",
        "pub(crate) fn is_admin_system_configs_root",
    ] {
        assert!(
            system_shared_paths.contains(pattern),
            "system/shared/paths.rs should own {pattern}"
        );
    }
    for pattern in [
        "admin_oauth_provider_type_from_path",
        "admin_oauth_test_provider_type_from_path",
    ] {
        assert!(
            !system_shared_paths.contains(pattern),
            "system/shared/paths.rs should not own auth oauth path helper {pattern}"
        );
    }

    let admin_shared_payloads =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/shared/payloads.rs");
    assert!(
        !admin_shared_payloads.contains("pub(crate) struct AdminOAuthProviderUpsertRequest"),
        "handlers/admin/shared/payloads.rs should not own AdminOAuthProviderUpsertRequest"
    );

    let auth_oauth_config =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/auth/oauth_config.rs");
    assert!(
        auth_oauth_config.contains("pub(crate) struct AdminOAuthProviderUpsertRequest"),
        "auth/oauth_config.rs should own AdminOAuthProviderUpsertRequest"
    );
    for pattern in [
        "pub(crate) fn admin_oauth_provider_type_from_path",
        "pub(crate) fn admin_oauth_test_provider_type_from_path",
    ] {
        assert!(
            auth_oauth_config.contains(pattern),
            "auth/oauth_config.rs should own {pattern}"
        );
    }
    assert!(
        !auth_oauth_config.contains("pub(crate) fn build_proxy_error_response"),
        "auth/oauth_config.rs should not own build_proxy_error_response"
    );
    assert!(
        !workspace_file_exists("apps/aether-gateway/src/handlers/admin/system/shared/payloads.rs"),
        "system/shared/payloads.rs should be removed after oauth payload ownership moves to auth"
    );

    let admin_shared_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/shared/mod.rs");
    assert!(
        admin_shared_mod.contains("mod proxy_errors;")
            && admin_shared_mod
                .contains("pub(crate) use self::proxy_errors::build_proxy_error_response;"),
        "handlers/admin/shared/mod.rs should expose shared admin proxy error builder"
    );
    let admin_shared_proxy_errors =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/shared/proxy_errors.rs");
    assert!(
        admin_shared_proxy_errors.contains("pub(crate) fn build_proxy_error_response"),
        "handlers/admin/shared/proxy_errors.rs should own build_proxy_error_response"
    );
}

#[test]
fn admin_handlers_expose_real_subdomains_without_facade() {
    let admin_mod = read_workspace_file("apps/aether-gateway/src/handlers/admin/mod.rs");
    assert!(
        !admin_mod.contains("pub(crate) use self::"),
        "handlers/admin/mod.rs should stay as pure module wiring after facade removal"
    );

    for pattern in [
        "pub(crate) mod auth;",
        "pub(crate) mod endpoint;",
        "pub(crate) mod features;",
        "pub(crate) mod observability;",
        "pub(crate) mod provider;",
        "pub(crate) mod system;",
    ] {
        assert!(
            admin_mod.contains(pattern),
            "handlers/admin/mod.rs should expose admin subdomain module {pattern}"
        );
    }

    assert!(
        !workspace_file_exists("apps/aether-gateway/src/handlers/admin/facade.rs"),
        "handlers/admin/facade.rs should be removed after direct subdomain exposure"
    );
}
