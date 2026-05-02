use super::*;

#[test]
fn admin_external_usage_is_confined_to_admin_api() {
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root should resolve");
    let mut violations = Vec::new();

    for file in collect_workspace_rust_files("apps/aether-gateway/src") {
        let relative = file
            .canonicalize()
            .expect("workspace file should canonicalize")
            .strip_prefix(&workspace_root)
            .expect("workspace file should be under workspace root")
            .to_string_lossy()
            .replace('\\', "/");
        if relative == "apps/aether-gateway/src/admin_api.rs"
            || relative.starts_with("apps/aether-gateway/src/handlers/admin/")
            || relative.starts_with("apps/aether-gateway/src/tests/")
        {
            continue;
        }

        let source = std::fs::read_to_string(&file).expect("source file should be readable");
        if source.contains("crate::handlers::admin::")
            || source.contains("use crate::handlers::admin::")
        {
            violations.push(relative);
        }
    }

    assert!(
        violations.is_empty(),
        "gateway code outside admin_api.rs should not directly depend on handlers::admin internals:\n{}",
        violations.join("\n")
    );
}

#[test]
fn admin_wrapped_state_owns_api_key_and_proxy_capabilities() {
    let admin_request =
        read_workspace_module_tree("apps/aether-gateway/src/handlers/admin/request/mod.rs");
    for pattern in [
        "pub(crate) fn has_auth_api_key_writer(&self) -> bool",
        "pub(crate) fn encryption_key(&self) -> Option<&str>",
        "pub(crate) fn encrypt_catalog_secret_with_fallbacks(&self, secret: &str) -> Option<String>",
        "pub(crate) fn decrypt_catalog_secret_with_fallbacks(",
        "pub(crate) async fn add_admin_security_blacklist(",
        "pub(crate) async fn list_auth_api_key_snapshots_by_ids(",
        "pub(crate) async fn list_auth_api_key_export_records_by_user_ids(",
        "pub(crate) async fn list_auth_api_key_export_standalone_records_page(",
        "pub(crate) async fn count_auth_api_key_export_standalone_records(",
        "pub(crate) async fn find_auth_api_key_export_standalone_record_by_id(",
        "pub(crate) async fn create_user_api_key(",
        "pub(crate) async fn create_standalone_api_key(",
        "pub(crate) async fn resolve_transport_proxy_snapshot_with_tunnel_affinity(",
        "pub(crate) async fn update_user_api_key_basic(",
        "pub(crate) async fn update_standalone_api_key_basic(",
        "pub(crate) async fn set_standalone_api_key_active(",
        "pub(crate) async fn set_user_api_key_locked(",
        "pub(crate) async fn set_user_api_key_allowed_providers(",
        "pub(crate) async fn summarize_usage_total_tokens_by_api_key_ids(",
        "pub(crate) async fn delete_user_api_key(",
        "pub(crate) async fn delete_standalone_api_key(",
    ] {
        assert!(
            admin_request.contains(pattern),
            "handlers/admin/request/mod.rs should expose admin state capability {pattern}"
        );
    }

    for path in [
        "apps/aether-gateway/src/handlers/admin/auth/api_keys/mutation_routes.rs",
        "apps/aether-gateway/src/handlers/admin/auth/oauth_config.rs",
        "apps/aether-gateway/src/handlers/admin/auth/ldap/builders.rs",
        "apps/aether-gateway/src/handlers/admin/users/api_keys/responses/create.rs",
        "apps/aether-gateway/src/handlers/admin/users/api_keys/responses/update.rs",
        "apps/aether-gateway/src/handlers/admin/users/api_keys/responses/delete.rs",
        "apps/aether-gateway/src/handlers/admin/users/api_keys/responses/toggle_lock.rs",
        "apps/aether-gateway/src/handlers/admin/users/api_keys/responses/list.rs",
        "apps/aether-gateway/src/handlers/admin/users/api_keys/responses/reveal.rs",
        "apps/aether-gateway/src/handlers/admin/features/gemini_files/upload/stage.rs",
        "apps/aether-gateway/src/handlers/admin/system/proxy_nodes.rs",
    ] {
        let contents = read_workspace_file(path);
        assert!(
            !contents.contains("state.data.has_auth_api_key_writer()"),
            "{path} should use AdminAppState capability instead of raw state.data.has_auth_api_key_writer()"
        );
        assert!(
            !contents.contains("state.data.has_proxy_node_reader()"),
            "{path} should use AdminAppState capability instead of raw state.data.has_proxy_node_reader()"
        );
        assert!(
            !contents.contains(".data\n        .list_auth_api_key_snapshots_by_ids(")
                && !contents.contains(".data.list_auth_api_key_snapshots_by_ids("),
            "{path} should use AdminAppState snapshot capability instead of raw state.data.list_auth_api_key_snapshots_by_ids()"
        );
        assert!(
            !contents.contains("encrypt_catalog_secret_with_fallbacks(state.app(),"),
            "{path} should use AdminAppState encryption capability instead of raw state.app() encryption"
        );
        assert!(
            !contents.contains("decrypt_catalog_secret_with_fallbacks(state.app().encryption_key(),"),
            "{path} should use AdminAppState decryption capability instead of raw state.app().encryption_key()"
        );
        assert!(
            !contents.contains("resolve_transport_proxy_snapshot_with_tunnel_affinity(\n            state.app(),")
                && !contents.contains("resolve_transport_proxy_snapshot_with_tunnel_affinity(state.app(),"),
            "{path} should use AdminAppState proxy capability instead of raw state.app() transport proxy resolution"
        );
    }
}

#[test]
fn admin_wrapped_state_owns_billing_capabilities() {
    let admin_request =
        read_workspace_module_tree("apps/aether-gateway/src/handlers/admin/request/mod.rs");
    for pattern in [
        "pub(crate) fn has_postgres_pool(&self) -> bool",
        "pub(crate) async fn list_admin_billing_collectors(",
        "pub(crate) async fn read_admin_billing_collector(",
        "pub(crate) async fn create_admin_billing_collector(",
        "pub(crate) async fn update_admin_billing_collector(",
        "pub(crate) async fn apply_admin_billing_preset(",
        "pub(crate) async fn list_admin_billing_rules(",
        "pub(crate) async fn read_admin_billing_rule(",
        "pub(crate) async fn create_admin_billing_rule(",
        "pub(crate) async fn update_admin_billing_rule(",
        "pub(crate) async fn list_admin_wallets(",
        "pub(crate) async fn list_admin_wallet_ledger(",
        "pub(crate) async fn list_admin_wallet_refund_requests(",
        "pub(crate) async fn list_admin_wallet_transactions(",
        "pub(crate) async fn list_admin_wallet_refunds(",
        "pub(crate) async fn list_admin_payment_orders(",
        "pub(crate) async fn list_admin_payment_callbacks(",
        "pub(crate) async fn read_admin_payment_order(",
        "pub(crate) async fn admin_expire_payment_order(",
        "pub(crate) async fn admin_credit_payment_order(",
        "pub(crate) async fn admin_fail_payment_order(",
        "pub(crate) async fn admin_adjust_wallet_balance(",
        "pub(crate) async fn admin_create_manual_wallet_recharge(",
        "pub(crate) async fn admin_process_wallet_refund(",
        "pub(crate) async fn admin_complete_wallet_refund(",
        "pub(crate) async fn admin_fail_wallet_refund(",
    ] {
        assert!(
            admin_request.contains(pattern),
            "handlers/admin/request/mod.rs should expose billing capability {pattern}"
        );
    }
}

#[test]
fn admin_wrapped_state_owns_observability_capabilities() {
    let admin_request =
        read_workspace_module_tree("apps/aether-gateway/src/handlers/admin/request/mod.rs");
    for pattern in [
        "pub(crate) fn has_auth_api_key_data_reader(&self) -> bool",
        "pub(crate) fn has_user_data_reader(&self) -> bool",
        "pub(crate) async fn list_provider_catalog_providers(",
        "pub(crate) async fn aggregate_finalized_request_candidate_timeline_by_endpoint_ids_since(",
        "pub(crate) async fn read_recent_request_candidates(",
        "pub(crate) fn provider_key_rpm_reset_at(",
        "pub(crate) async fn update_provider_catalog_key_health_state(",
        "pub(crate) async fn list_usage_audits(",
        "pub(crate) async fn list_users_by_ids(",
    ] {
        assert!(
            admin_request.contains(pattern),
            "handlers/admin/request/mod.rs should expose observability capability {pattern}"
        );
    }
    for pattern in [
        "pub(crate) async fn list_admin_usage_for_range(",
        "pub(crate) async fn list_admin_usage_for_optional_range(",
    ] {
        assert!(
            !admin_request.contains(pattern),
            "handlers/admin/request/mod.rs should not expose deprecated unbounded usage helper {pattern}"
        );
    }
}

#[test]
fn admin_wrapped_state_owns_provider_oauth_capabilities() {
    let admin_request =
        read_workspace_module_tree("apps/aether-gateway/src/handlers/admin/request/mod.rs");
    for pattern in [
        "pub(crate) fn cloned_app(&self) -> AppState",
        "pub(crate) async fn save_provider_oauth_state(",
        "pub(crate) async fn consume_provider_oauth_state(",
        "pub(crate) async fn exchange_admin_provider_oauth_code(",
        "pub(crate) async fn exchange_admin_provider_oauth_refresh_token(",
        "pub(crate) async fn save_provider_oauth_batch_task_payload(",
        "pub(crate) async fn read_provider_oauth_batch_task_payload(",
        "pub(crate) async fn save_provider_oauth_device_session(",
        "pub(crate) async fn read_provider_oauth_device_session(",
        "pub(crate) async fn register_admin_kiro_device_oidc_client(",
        "pub(crate) async fn start_admin_kiro_device_authorization(",
        "pub(crate) async fn poll_admin_kiro_device_token(",
        "pub(crate) async fn find_duplicate_provider_oauth_key(",
        "pub(crate) async fn create_provider_oauth_catalog_key(",
        "pub(crate) async fn update_existing_provider_oauth_catalog_key(",
        "pub(crate) async fn refresh_provider_oauth_account_state_after_update(",
        "pub(crate) async fn update_provider_catalog_key_oauth_credentials(",
    ] {
        assert!(
            admin_request.contains(pattern),
            "handlers/admin/request/mod.rs should expose provider oauth capability {pattern}"
        );
    }

    for path in [
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/start.rs",
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/import.rs",
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/tasks.rs",
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/refresh/request.rs",
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/device/authorize.rs",
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/device/poll.rs",
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/complete/key.rs",
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/complete/provider.rs",
    ] {
        let contents = read_workspace_file(path);
        assert!(
            !contents.contains("state.app()"),
            "{path} should use AdminAppState oauth capability instead of raw state.app()"
        );
    }
}

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

    assert!(
        !workspace_file_exists("apps/aether-gateway/src/handlers/admin/provider/shared/paths.rs"),
        "provider/shared/paths.rs should be replaced by split path-owner modules"
    );

    let provider_paths_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/shared/paths/mod.rs");
    for pattern in [
        "pub(crate) use self::crud::{",
        "pub(crate) use self::oauth::{",
        "pub(crate) use self::ops::{",
    ] {
        assert!(
            provider_paths_mod.contains(pattern),
            "provider/shared/paths/mod.rs should re-export split path owners through {pattern}"
        );
    }

    let provider_crud_paths =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/shared/paths/crud.rs");
    assert!(
        provider_crud_paths.contains("pub(crate) fn admin_provider_id_for_manage_path"),
        "provider/shared/paths/crud.rs should own admin_provider_id_for_manage_path"
    );

    let provider_oauth_paths = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/shared/paths/oauth.rs",
    );
    assert!(
        provider_oauth_paths.contains("pub(crate) fn admin_provider_oauth_start_key_id"),
        "provider/shared/paths/oauth.rs should own admin_provider_oauth_start_key_id"
    );

    let provider_ops_paths =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/shared/paths/ops.rs");
    assert!(
        provider_ops_paths.contains("pub(crate) fn admin_provider_ops_architecture_id_from_path"),
        "provider/shared/paths/ops.rs should own admin_provider_ops_architecture_id_from_path"
    );

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

    for pattern in [
        "mod announcements;",
        "pub(super) mod auth;",
        "pub(super) mod endpoint;",
        "pub(super) mod features;",
        "pub(super) mod observability;",
        "pub(super) mod provider;",
        "pub(super) mod request;",
        "pub(super) mod routes;",
        "mod system;",
    ] {
        assert!(
            admin_mod.contains(pattern),
            "handlers/admin/mod.rs should expose admin subdomain module {pattern}"
        );
    }

    for forbidden in [
        "pub(crate) mod announcements;",
        "pub(crate) mod auth;",
        "pub(crate) mod endpoint;",
        "pub(crate) mod features;",
        "pub(crate) mod observability;",
        "pub(crate) mod provider;",
        "pub(crate) mod system;",
    ] {
        assert!(
            !admin_mod.contains(forbidden),
            "handlers/admin/mod.rs should keep non-api subdomains private for {forbidden}"
        );
    }

    for pattern in [
        "pub(crate) use self::request::{",
        "AdminAppState",
        "AdminRequestContext",
        "AdminRouteRequest",
        "AdminRouteResponse",
        "AdminRouteResult",
        "pub(crate) use self::routes::maybe_build_local_admin_response;",
        "pub(crate) use self::provider::{",
        "maybe_build_local_admin_provider_oauth_response",
        "maybe_build_local_admin_providers_response",
    ] {
        assert!(
            admin_mod.contains(pattern),
            "handlers/admin/mod.rs should expose the crate-facing admin seam {pattern}"
        );
    }

    assert!(
        !workspace_file_exists("apps/aether-gateway/src/handlers/admin/facade.rs"),
        "handlers/admin/facade.rs should be removed after direct subdomain exposure"
    );
}

#[test]
fn ai_serving_external_consumers_use_single_api_facade() {
    let ai_serving_mod = read_workspace_file("apps/aether-gateway/src/ai_serving/mod.rs");
    for pattern in [
        "mod adaptation;",
        "mod finalize;",
        "mod planner;",
        "pub(crate) mod transport;",
        "pub(crate) use self::finalize::internal::{",
        "pub(crate) use self::planner::{",
        "pub(crate) use crate::execution_runtime::{ConversionMode, ExecutionStrategy};",
        "pub(crate) async fn resolve_execution_runtime_auth_context(",
        "pub(crate) fn maybe_build_local_sync_finalize_response(",
    ] {
        assert!(
            ai_serving_mod.contains(pattern),
            "ai_serving/mod.rs should expose only the crate-facing ai_serving seam for {pattern}"
        );
    }

    for forbidden in [
        "pub(super) mod adaptation;",
        "pub(super) mod finalize;",
        "pub(super) mod planner;",
        "pub(crate) mod adaptation;",
        "pub(crate) mod contracts;",
        "mod contracts;",
        "pub(crate) mod conversion;",
        "mod conversion;",
        "pub(crate) mod finalize;",
        "pub(crate) mod planner;",
        "pub(super) mod api;",
    ] {
        assert!(
            !ai_serving_mod.contains(forbidden),
            "ai_serving/mod.rs should not expose internal module {forbidden}"
        );
    }

    for path in [
        "apps/aether-gateway/src/executor/orchestration.rs",
        "apps/aether-gateway/src/execution_runtime/sync/execution.rs",
        "apps/aether-gateway/src/execution_runtime/stream/execution.rs",
        "apps/aether-gateway/src/execution_runtime/submission.rs",
        "apps/aether-gateway/src/execution_runtime/tests.rs",
        "apps/aether-gateway/src/lib.rs",
    ] {
        let contents = read_workspace_file(path);
        assert!(
            contents.contains("ai_serving::api"),
            "{path} should use the ai_serving::api facade"
        );
        for forbidden in [
            "crate::ai_serving::planner::",
            "crate::ai_serving::contracts::",
            "crate::ai_serving::adaptation::private_envelope",
            "crate::ai_serving::conversion::",
        ] {
            assert!(
                !contents.contains(forbidden),
                "{path} should not bypass ai_serving::api through {forbidden}"
            );
        }
    }
}

#[test]
fn crate_root_exposes_real_admin_and_ai_serving_facades() {
    let lib_rs = read_workspace_file("apps/aether-gateway/src/lib.rs");
    for pattern in ["mod admin_api;", "mod ai_serving;"] {
        assert!(
            lib_rs.contains(pattern),
            "lib.rs should register crate root facade module {pattern}"
        );
    }
    let forbidden = "pub(crate) use self::handlers::admin_api;";
    assert!(
        !lib_rs.contains(forbidden),
        "lib.rs should not keep alias-only facade wiring {forbidden}"
    );

    let admin_api = read_workspace_file("apps/aether-gateway/src/admin_api.rs");
    assert!(
        admin_api.contains("pub(crate) use crate::handlers::admin::{"),
        "admin_api.rs should own the crate root admin facade instead of re-exporting handlers/admin_api"
    );

    let handlers_mod = read_workspace_file("apps/aether-gateway/src/handlers/mod.rs");
    assert!(
        handlers_mod.contains("pub(super) mod admin;"),
        "handlers/mod.rs should expose admin only to the crate root boundary"
    );
    assert!(
        !handlers_mod.contains("pub(super) mod admin_api;"),
        "handlers/mod.rs should not keep a separate handlers/admin_api facade module"
    );
    assert!(
        !handlers_mod.contains("pub(crate) use self::admin::api as admin_api;"),
        "handlers/mod.rs should not keep alias-only admin_api wiring after crate root facade extraction"
    );

    let ai_serving_api_mod = read_workspace_file("apps/aether-gateway/src/ai_serving/api.rs");
    assert!(
        ai_serving_api_mod.contains("use crate::ai_serving::{is_json_request, GatewayControlDecision};"),
        "ai_serving/api.rs should depend on the crate-facing ai_serving seam instead of deep internal modules"
    );
}

#[test]
fn gateway_ai_serving_api_module_delegates_pure_ownership_to_format_crate() {
    let gateway_api = read_workspace_file("apps/aether-gateway/src/ai_serving/api.rs");
    assert!(
        gateway_api.contains("pub(crate) use aether_ai_formats::api::{"),
        "ai_serving/api.rs should re-export pure ownership through aether_ai_formats::api"
    );

    let format_crate_api = read_workspace_file("crates/aether-ai-formats/src/api.rs");
    for pattern in [
        "pub use crate::contracts::{",
        "pub use crate::provider_compat::kiro_stream::{",
        "pub use crate::provider_compat::private_envelope::{",
        "pub use crate::provider_compat::surfaces::{",
        "pub use crate::request::common::{",
        "pub use crate::request::route::{",
        "pub use crate::response::common::{",
        "pub use crate::response::error_body::{",
        "pub use aether_ai_formats::{",
        "pub use aether_ai_formats::protocol::conversion::request::{",
        "pub use aether_ai_formats::protocol::conversion::response::{",
    ] {
        assert!(
            format_crate_api.contains(pattern),
            "aether-ai-formats/src/api.rs should expose crate facade seam {pattern}"
        );
    }
}

#[test]
fn admin_proxy_uses_single_admin_routes_entrypoint() {
    let proxy_local = read_workspace_file("apps/aether-gateway/src/handlers/proxy/local.rs");
    assert!(
        proxy_local.contains("admin_api::maybe_build_local_admin_response("),
        "handlers/proxy/local.rs should delegate admin dispatch through crate root admin_api facade"
    );
    assert!(
        proxy_local.contains("admin_api::AdminRouteRequest::new("),
        "handlers/proxy/local.rs should construct AdminRouteRequest through crate root admin_api facade"
    );
    let admin_api = read_workspace_file("apps/aether-gateway/src/admin_api.rs");
    for pattern in [
        "pub(crate) use crate::handlers::admin::{",
        "AdminAppState",
        "AdminRequestContext",
        "AdminRouteRequest",
        "AdminRouteResponse",
        "AdminRouteResult",
        "maybe_build_local_admin_response",
    ] {
        assert!(
            admin_api.contains(pattern),
            "admin_api.rs should own the public admin entry seam {pattern}"
        );
    }

    for forbidden in [
        "auth as admin_auth",
        "billing as admin_billing",
        "endpoint as admin_endpoint",
        "features as admin_features",
        "model as admin_model",
        "observability as admin_observability",
        "provider as admin_provider",
        "system as admin_system",
        "users as admin_users",
        "public::maybe_build_local_admin_announcements_response(",
        "admin_auth::maybe_build_local_admin_auth_response(",
        "admin_observability::maybe_build_local_admin_observability_response(",
        "admin_features::maybe_build_local_admin_features_response(",
        "admin_users::maybe_build_local_admin_users_response(",
        "admin_provider::maybe_build_local_admin_provider_oauth_response(",
        "admin_provider::maybe_build_local_admin_provider_response(",
        "admin_system::maybe_build_local_admin_core_response(",
        "admin_system::maybe_build_local_admin_system_response(",
        "admin_billing::maybe_build_local_admin_billing_routes_response(",
    ] {
        assert!(
            !proxy_local.contains(forbidden),
            "handlers/proxy/local.rs should not dispatch admin subdomains directly for {forbidden}"
        );
    }

    let admin_routes = read_workspace_file("apps/aether-gateway/src/handlers/admin/routes.rs");
    for pattern in [
        "use super::{",
        "pub(crate) async fn maybe_build_local_admin_response(",
        "request::AdminRouteRequest<'_>",
        ") -> request::AdminRouteResult {",
        "announcements::maybe_build_local_admin_announcements_response(",
        "auth::maybe_build_local_admin_auth_response(",
        "observability::maybe_build_local_admin_observability_response(",
        "features::maybe_build_local_admin_features_response(",
        "model::maybe_build_local_admin_model_response(",
        "provider::maybe_build_local_admin_provider_response(",
        "system::maybe_build_local_admin_system_response(",
        "billing::maybe_build_local_admin_billing_routes_response(",
        "users::maybe_build_local_admin_users_response(",
        "endpoint::maybe_build_local_admin_endpoints_response(",
    ] {
        assert!(
            admin_routes.contains(pattern),
            "handlers/admin/routes.rs should own admin proxy dispatch seam {pattern}"
        );
    }

    for forbidden in [
        "use super::super::public;",
        "public::maybe_build_local_admin_announcements_response(",
        "auth::maybe_build_local_admin_security_response(",
        "auth::maybe_build_local_admin_api_keys_response(",
        "auth::maybe_build_local_admin_ldap_response(",
        "observability::maybe_build_local_admin_stats_response(",
        "observability::maybe_build_local_admin_monitoring_response(",
        "observability::maybe_build_local_admin_usage_response(",
        "model::maybe_build_local_admin_global_models_response(",
        "model::maybe_build_local_admin_model_catalog_response(",
        "features::maybe_build_local_admin_video_tasks_response(",
        "features::maybe_build_local_admin_gemini_files_response(",
        "provider::maybe_build_local_admin_provider_oauth_response(",
        "provider::maybe_build_local_admin_provider_models_response(",
        "provider::maybe_build_local_admin_providers_response(",
        "provider::maybe_build_local_admin_provider_ops_response(",
        "provider::maybe_build_local_admin_provider_query_response(",
        "provider::maybe_build_local_admin_provider_strategy_response(",
        "billing::maybe_build_local_admin_billing_response(",
        "billing::maybe_build_local_admin_payments_response(",
        "billing::maybe_build_local_admin_wallets_response(",
    ] {
        assert!(
            !admin_routes.contains(forbidden),
            "handlers/admin/routes.rs should not dispatch provider or billing internals directly for {forbidden}"
        );
    }

    let admin_request =
        read_workspace_module_tree("apps/aether-gateway/src/handlers/admin/request/mod.rs");
    for pattern in [
        "pub(crate) struct AdminAppState<'a>",
        "pub(crate) fn new(app: &'a AppState) -> Self",
        "pub(crate) fn app(&self) -> &AppState",
        "pub(crate) fn has_provider_catalog_data_reader(&self) -> bool",
        "pub(crate) fn has_provider_catalog_data_writer(&self) -> bool",
        "pub(crate) fn has_request_candidate_data_reader(&self) -> bool",
        "pub(crate) fn has_management_token_reader(&self) -> bool",
        "pub(crate) fn has_management_token_writer(&self) -> bool",
        "pub(crate) fn has_global_model_data_reader(&self) -> bool",
        "pub(crate) fn has_usage_data_reader(&self) -> bool",
        "pub(crate) fn has_auth_module_writer(&self) -> bool",
        "pub(crate) async fn get_ldap_module_config(",
        "pub(crate) async fn upsert_ldap_module_config(",
        "pub(crate) async fn count_active_local_admin_users_with_valid_password(",
        "pub(crate) async fn list_oauth_provider_configs(",
        "pub(crate) async fn get_oauth_provider_config(",
        "pub(crate) async fn upsert_oauth_provider_config(",
        "pub(crate) async fn delete_oauth_provider_config(",
        "pub(crate) async fn get_management_token_with_user(",
        "pub(crate) async fn delete_management_token(",
        "pub(crate) async fn remove_admin_security_blacklist(",
        "pub(crate) async fn admin_security_blacklist_stats(",
        "pub(crate) async fn list_admin_security_blacklist(",
        "pub(crate) async fn add_admin_security_whitelist(",
        "pub(crate) async fn remove_admin_security_whitelist(",
        "pub(crate) async fn list_admin_security_whitelist(&self) -> Result<Vec<String>, GatewayError>",
        "pub(crate) fn mark_provider_key_rpm_reset(&self, key_id: &str, now_unix_secs: u64)",
        "pub(crate) async fn list_proxy_nodes(",
        "pub(crate) async fn find_proxy_node(",
        "pub(crate) async fn read_provider_catalog_endpoints_by_ids(",
        "pub(crate) async fn count_distinct_video_task_users(",
        "pub(crate) struct AdminRequestContext<'a>",
        "pub(crate) fn new(context: &'a GatewayPublicRequestContext) -> Self",
        "pub(crate) fn decision(&self) -> Option<&GatewayControlDecision>",
        "pub(crate) fn method(&self) -> &Method",
        "pub(crate) fn path(&self) -> &str",
        "pub(crate) fn query_string(&self) -> Option<&str>",
        "pub(crate) fn public(&self) -> &GatewayPublicRequestContext",
        "impl<'a> Deref for AdminRequestContext<'a>",
        "pub(crate) type AdminRouteResponse",
        "pub(crate) type AdminRouteResult",
        "pub(crate) struct AdminRouteRequest<'a>",
        "pub(crate) fn new(",
        "state: AdminAppState<'a>",
        "request_context: AdminRequestContext<'a>",
        "request_body: Option<&'a Bytes>",
        "pub(crate) fn state(self) -> AdminAppState<'a>",
        "pub(crate) fn request_context(self) -> AdminRequestContext<'a>",
        "pub(crate) fn request_body(self) -> Option<&'a Bytes>",
    ] {
        assert!(
            admin_request.contains(pattern),
            "handlers/admin/request/mod.rs should own unified admin request injection field {pattern}"
        );
    }
}

#[test]
fn admin_second_layer_route_seams_use_wrapped_request_types() {
    for file in [
        "apps/aether-gateway/src/handlers/admin/auth/security.rs",
        "apps/aether-gateway/src/handlers/admin/auth/api_keys/mod.rs",
        "apps/aether-gateway/src/handlers/admin/auth/ldap/mod.rs",
        "apps/aether-gateway/src/handlers/admin/auth/ldap/routes.rs",
        "apps/aether-gateway/src/handlers/admin/auth/oauth_routes.rs",
        "apps/aether-gateway/src/handlers/admin/billing/mod.rs",
        "apps/aether-gateway/src/handlers/admin/billing/collectors/mod.rs",
        "apps/aether-gateway/src/handlers/admin/billing/payments/mod.rs",
        "apps/aether-gateway/src/handlers/admin/billing/payments/routes.rs",
        "apps/aether-gateway/src/handlers/admin/billing/presets/mod.rs",
        "apps/aether-gateway/src/handlers/admin/billing/wallets/mod.rs",
        "apps/aether-gateway/src/handlers/admin/billing/wallets/routes.rs",
        "apps/aether-gateway/src/handlers/admin/endpoint/health.rs",
        "apps/aether-gateway/src/handlers/admin/endpoint/rpm.rs",
        "apps/aether-gateway/src/handlers/admin/features/video_tasks/mod.rs",
        "apps/aether-gateway/src/handlers/admin/features/video_tasks/routes.rs",
        "apps/aether-gateway/src/handlers/admin/features/gemini_files/mod.rs",
        "apps/aether-gateway/src/handlers/admin/model/catalog_routes.rs",
        "apps/aether-gateway/src/handlers/admin/model/global_models/routes/core/mod.rs",
        "apps/aether-gateway/src/handlers/admin/observability/stats/mod.rs",
        "apps/aether-gateway/src/handlers/admin/observability/stats/analytics_routes.rs",
        "apps/aether-gateway/src/handlers/admin/observability/stats/cost_routes.rs",
        "apps/aether-gateway/src/handlers/admin/observability/stats/leaderboard_routes.rs",
        "apps/aether-gateway/src/handlers/admin/observability/stats/provider_quota_routes.rs",
        "apps/aether-gateway/src/handlers/admin/observability/monitoring/mod.rs",
        "apps/aether-gateway/src/handlers/admin/observability/usage/mod.rs",
        "apps/aether-gateway/src/handlers/admin/provider/endpoint_keys.rs",
        "apps/aether-gateway/src/handlers/admin/provider/endpoints_admin/mod.rs",
        "apps/aether-gateway/src/handlers/admin/provider/models/mod.rs",
        "apps/aether-gateway/src/handlers/admin/provider/ops/mod.rs",
        "apps/aether-gateway/src/handlers/admin/provider/pool_admin/mod.rs",
        "apps/aether-gateway/src/handlers/admin/provider/query/mod.rs",
        "apps/aether-gateway/src/handlers/admin/provider/query/routes.rs",
        "apps/aether-gateway/src/handlers/admin/provider/strategy/mod.rs",
        "apps/aether-gateway/src/handlers/admin/provider/strategy/routes.rs",
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/mod.rs",
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/refresh.rs",
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/device/mod.rs",
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/complete/mod.rs",
        "apps/aether-gateway/src/handlers/admin/system/core/mod.rs",
        "apps/aether-gateway/src/handlers/admin/system/adaptive/mod.rs",
        "apps/aether-gateway/src/handlers/admin/system/adaptive/routes.rs",
        "apps/aether-gateway/src/handlers/admin/system/management_tokens.rs",
        "apps/aether-gateway/src/handlers/admin/system/modules.rs",
        "apps/aether-gateway/src/handlers/admin/system/proxy_nodes.rs",
        "apps/aether-gateway/src/handlers/admin/users/routes.rs",
    ] {
        let contents = read_workspace_file(file);
        assert!(
            contents.contains("state: &AdminAppState<'_>,"),
            "{file} should accept AdminAppState at the second-layer admin seam",
        );
        assert!(
            contents.contains("request_context: &AdminRequestContext<'_>,"),
            "{file} should accept AdminRequestContext at the second-layer admin seam",
        );
        assert!(
            !contents.contains("let state = state.app();"),
            "{file} should not expose raw AppState as a second-layer local variable",
        );
        assert!(
            !contents.contains("let app_state = state.app();"),
            "{file} should not expose raw AppState aliasing at the second-layer admin seam",
        );
    }

    let admin_request =
        read_workspace_module_tree("apps/aether-gateway/src/handlers/admin/request/mod.rs");
    assert!(
        !admin_request.contains("impl<'a> Deref for AdminAppState<'a>"),
        "handlers/admin/request/mod.rs should not expose AdminAppState via implicit Deref<AppState>",
    );
}

#[test]
fn admin_route_adjacent_owners_use_wrapped_state_types() {
    for file in [
        "apps/aether-gateway/src/handlers/admin/auth/ldap/builders.rs",
        "apps/aether-gateway/src/handlers/admin/auth/api_keys/shared.rs",
        "apps/aether-gateway/src/handlers/admin/auth/api_keys/mutation_routes.rs",
        "apps/aether-gateway/src/handlers/admin/auth/api_keys/read_routes.rs",
        "apps/aether-gateway/src/handlers/admin/auth/oauth_config.rs",
        "apps/aether-gateway/src/handlers/admin/billing/collectors/reads.rs",
        "apps/aether-gateway/src/handlers/admin/billing/collectors/writes.rs",
        "apps/aether-gateway/src/handlers/admin/billing/payments/callbacks.rs",
        "apps/aether-gateway/src/handlers/admin/billing/payments/orders.rs",
        "apps/aether-gateway/src/handlers/admin/billing/presets/apply.rs",
        "apps/aether-gateway/src/handlers/admin/billing/rules.rs",
        "apps/aether-gateway/src/handlers/admin/billing/wallets/reads/detail.rs",
        "apps/aether-gateway/src/handlers/admin/billing/wallets/reads/ledger.rs",
        "apps/aether-gateway/src/handlers/admin/billing/wallets/reads/list.rs",
        "apps/aether-gateway/src/handlers/admin/billing/wallets/reads/refund_requests.rs",
        "apps/aether-gateway/src/handlers/admin/billing/wallets/reads/refunds.rs",
        "apps/aether-gateway/src/handlers/admin/billing/wallets/reads/transactions.rs",
        "apps/aether-gateway/src/handlers/admin/billing/wallets/mutations/adjust.rs",
        "apps/aether-gateway/src/handlers/admin/billing/wallets/mutations/complete_refund.rs",
        "apps/aether-gateway/src/handlers/admin/billing/wallets/mutations/fail_refund.rs",
        "apps/aether-gateway/src/handlers/admin/billing/wallets/mutations/process_refund.rs",
        "apps/aether-gateway/src/handlers/admin/billing/wallets/mutations/recharge.rs",
        "apps/aether-gateway/src/handlers/admin/billing/wallets/shared/payloads.rs",
        "apps/aether-gateway/src/handlers/admin/model/global/helpers.rs",
        "apps/aether-gateway/src/handlers/admin/model/global/payloads.rs",
        "apps/aether-gateway/src/handlers/admin/model/global/providers.rs",
        "apps/aether-gateway/src/handlers/admin/model/write.rs",
        "apps/aether-gateway/src/handlers/admin/users/lifecycle/support.rs",
        "apps/aether-gateway/src/handlers/admin/provider/query/models.rs",
        "apps/aether-gateway/src/handlers/admin/provider/strategy/builders.rs",
        "apps/aether-gateway/src/handlers/admin/features/video_tasks/builders.rs",
        "apps/aether-gateway/src/handlers/admin/observability/stats/leaderboard.rs",
        "apps/aether-gateway/src/handlers/admin/provider/endpoints_admin/reads.rs",
        "apps/aether-gateway/src/handlers/admin/provider/models/payloads.rs",
        "apps/aether-gateway/src/handlers/admin/system/shared/configs.rs",
        "apps/aether-gateway/src/handlers/admin/system/shared/settings.rs",
        "apps/aether-gateway/src/handlers/admin/system/shared/modules.rs",
        "apps/aether-gateway/src/handlers/admin/system/shared/export/providers.rs",
        "apps/aether-gateway/src/handlers/admin/users/sessions.rs",
        "apps/aether-gateway/src/handlers/admin/users/api_keys/responses/create.rs",
        "apps/aether-gateway/src/handlers/admin/users/api_keys/responses/delete.rs",
        "apps/aether-gateway/src/handlers/admin/users/api_keys/responses/list.rs",
        "apps/aether-gateway/src/handlers/admin/users/api_keys/responses/reveal.rs",
        "apps/aether-gateway/src/handlers/admin/users/api_keys/responses/toggle_lock.rs",
        "apps/aether-gateway/src/handlers/admin/users/api_keys/responses/update.rs",
        "apps/aether-gateway/src/handlers/admin/features/gemini_files/read_routes.rs",
        "apps/aether-gateway/src/handlers/admin/features/gemini_files/upload/mod.rs",
        "apps/aether-gateway/src/handlers/admin/features/gemini_files/upload/request.rs",
        "apps/aether-gateway/src/handlers/admin/features/gemini_files/upload/support.rs",
        "apps/aether-gateway/src/handlers/admin/model/global_models/routes/core/reads.rs",
        "apps/aether-gateway/src/handlers/admin/model/global_models/routes/core/writes.rs",
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/start.rs",
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/import.rs",
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/tasks.rs",
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/refresh/request.rs",
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/device/authorize.rs",
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/device/poll.rs",
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/complete/key.rs",
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/complete/provider.rs",
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/batch/orchestration.rs",
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/batch/task.rs",
    ] {
        let contents = read_workspace_file(file);
        assert!(
            contents.contains("state: &AdminAppState<'_>,"),
            "{file} should accept AdminAppState at the route-adjacent admin owner layer",
        );
        assert!(
            !contents.contains("state: &AppState,"),
            "{file} should not keep raw AppState in route-adjacent owner signatures",
        );
    }

    for file in [
        "apps/aether-gateway/src/handlers/admin/provider/endpoints_admin/create.rs",
        "apps/aether-gateway/src/handlers/admin/provider/endpoints_admin/update.rs",
        "apps/aether-gateway/src/handlers/admin/provider/endpoints_admin/delete.rs",
        "apps/aether-gateway/src/handlers/admin/provider/endpoints_admin/list.rs",
        "apps/aether-gateway/src/handlers/admin/provider/endpoints_admin/detail.rs",
        "apps/aether-gateway/src/handlers/admin/provider/endpoints_admin/defaults.rs",
        "apps/aether-gateway/src/handlers/admin/provider/models/list.rs",
        "apps/aether-gateway/src/handlers/admin/provider/models/detail.rs",
        "apps/aether-gateway/src/handlers/admin/provider/models/create.rs",
        "apps/aether-gateway/src/handlers/admin/provider/models/update.rs",
        "apps/aether-gateway/src/handlers/admin/provider/models/delete.rs",
        "apps/aether-gateway/src/handlers/admin/provider/models/batch.rs",
        "apps/aether-gateway/src/handlers/admin/provider/models/available_source.rs",
        "apps/aether-gateway/src/handlers/admin/provider/models/assign_global.rs",
        "apps/aether-gateway/src/handlers/admin/provider/models/import.rs",
        "apps/aether-gateway/src/handlers/admin/users/lifecycle/reads.rs",
        "apps/aether-gateway/src/handlers/admin/users/lifecycle/create.rs",
        "apps/aether-gateway/src/handlers/admin/users/lifecycle/update.rs",
        "apps/aether-gateway/src/handlers/admin/users/lifecycle/delete.rs",
        "apps/aether-gateway/src/handlers/admin/users/api_keys/responses/create.rs",
        "apps/aether-gateway/src/handlers/admin/users/api_keys/responses/delete.rs",
        "apps/aether-gateway/src/handlers/admin/users/api_keys/responses/list.rs",
        "apps/aether-gateway/src/handlers/admin/users/api_keys/responses/reveal.rs",
        "apps/aether-gateway/src/handlers/admin/users/api_keys/responses/toggle_lock.rs",
        "apps/aether-gateway/src/handlers/admin/users/api_keys/responses/update.rs",
        "apps/aether-gateway/src/handlers/admin/features/gemini_files/read_routes.rs",
        "apps/aether-gateway/src/handlers/admin/features/gemini_files/upload/mod.rs",
        "apps/aether-gateway/src/handlers/admin/features/gemini_files/upload/request.rs",
        "apps/aether-gateway/src/handlers/admin/features/gemini_files/upload/support.rs",
    ] {
        let contents = read_workspace_file(file);
        assert!(
            contents.contains("state: &AdminAppState<'_>,"),
            "{file} should accept AdminAppState at the route-owner layer",
        );
        assert!(
            contents.contains("request_context: &AdminRequestContext<'_>,")
                || contents.contains("_state: &AdminAppState<'_>,"),
            "{file} should accept wrapped admin request/state types at the route-owner layer",
        );
        assert!(
            !contents.contains("state: &AppState,"),
            "{file} should not keep raw AppState in route-owner signatures",
        );
        assert!(
            !contents.contains("request_context: &GatewayPublicRequestContext,"),
            "{file} should not keep raw GatewayPublicRequestContext in route-owner signatures",
        );
    }

    for file in [
        "apps/aether-gateway/src/handlers/admin/endpoint/health_builders/status.rs",
        "apps/aether-gateway/src/handlers/admin/endpoint/health_builders/keys.rs",
        "apps/aether-gateway/src/handlers/admin/provider/write/reveal.rs",
        "apps/aether-gateway/src/handlers/admin/provider/write/keys/payload.rs",
    ] {
        let contents = read_workspace_file(file);
        assert!(
            contents.contains("state: &AdminAppState<'_>,"),
            "{file} should accept AdminAppState at the wrapped helper-owner layer",
        );
        assert!(
            !contents.contains("state: &AppState,"),
            "{file} should not keep raw AppState in helper-owner signatures",
        );
    }
}
