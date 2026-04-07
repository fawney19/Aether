use super::*;

#[test]
fn admin_system_and_endpoint_roots_stay_thin() {
    let system_mod = read_workspace_file("apps/aether-gateway/src/handlers/admin/system/mod.rs");
    for pattern in [
        "pub(super) use super::auth::{",
        "pub(super) use super::model::{",
        "pub(super) use super::provider::{",
    ] {
        assert!(
            !system_mod.contains(pattern),
            "handlers/admin/system/mod.rs should not act as a cross-domain re-export layer for {pattern}"
        );
    }
    for pattern in [
        "pub(crate) use self::adaptive::maybe_build_local_admin_adaptive_response;",
        "pub(crate) use self::core::maybe_build_local_admin_core_response;",
        "pub(crate) use self::management_tokens::maybe_build_local_admin_management_tokens_response;",
        "pub(crate) use self::modules::maybe_build_local_admin_modules_response;",
        "pub(crate) use self::proxy_nodes::maybe_build_local_admin_proxy_nodes_response;",
    ] {
        assert!(
            system_mod.contains(pattern),
            "handlers/admin/system/mod.rs should stay as a thin system subdomain router for {pattern}"
        );
    }
    assert!(
        system_mod.contains(
            "pub(crate) use crate::handlers::admin::provider::pool_admin::maybe_build_local_admin_pool_response;"
        ),
        "handlers/admin/system/mod.rs should delegate pool admin seam directly to provider::pool_admin"
    );

    assert!(
        !workspace_file_exists("apps/aether-gateway/src/handlers/admin/system/pool/mod.rs"),
        "handlers/admin/system/pool/mod.rs should be deleted once system root delegates pool admin directly to provider::pool_admin"
    );

    let endpoint_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/endpoint/mod.rs");
    for pattern in [
        "use self::extractors::{",
        "use self::health_builders::{",
        "use self::payloads::{",
    ] {
        assert!(
            !endpoint_mod.contains(pattern),
            "handlers/admin/endpoint/mod.rs should not re-export local helper seam {pattern}"
        );
    }
    assert!(
        endpoint_mod.contains(
            "pub(crate) use self::health_builders::build_admin_endpoint_health_status_payload;"
        ),
        "handlers/admin/endpoint/mod.rs should keep only the public health status payload seam"
    );

    let system_core_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/system/core/mod.rs");
    for pattern in [
        "maybe_build_local_admin_management_tokens_response",
        "maybe_build_local_admin_oauth_response",
        "maybe_build_local_admin_modules_response",
        "maybe_build_local_admin_model_catalog_response",
    ] {
        assert!(
            system_core_mod.contains(pattern),
            "handlers/admin/system/core/mod.rs should call the real owner {pattern}"
        );
    }
    for path in [
        "apps/aether-gateway/src/handlers/admin/system/core/management_tokens_routes.rs",
        "apps/aether-gateway/src/handlers/admin/system/core/model_routes.rs",
        "apps/aether-gateway/src/handlers/admin/system/core/modules_routes.rs",
        "apps/aether-gateway/src/handlers/admin/system/core/oauth_routes.rs",
    ] {
        assert!(
            !workspace_file_exists(path),
            "{path} should be deleted once system/core/mod.rs dispatches directly to real owners"
        );
    }

    let system_routes =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/system/core/system_routes.rs");
    assert!(
        !system_routes.contains("use crate::handlers::public::{"),
        "handlers/admin/system/core/system_routes.rs should not borrow system-owned route helpers from handlers/public"
    );
    assert!(
        !system_routes.contains("crate::handlers::admin::auth::build_proxy_error_response")
            && !system_routes.contains("use crate::handlers::admin::auth::build_proxy_error_response;"),
        "handlers/admin/system/core/system_routes.rs should not borrow proxy error builder from auth"
    );
    for pattern in [
        "build_admin_email_template_payload",
        "build_admin_email_templates_payload",
        "preview_admin_email_template",
        "reset_admin_email_template",
    ] {
        assert!(
            system_routes.contains(pattern),
            "handlers/admin/system/core/system_routes.rs should keep delegating through admin system shared helper {pattern}"
        );
    }

    assert!(
        endpoint_mod.contains(
            "endpoint_keys::maybe_build_local_admin_endpoints_keys_response"
        ),
        "handlers/admin/endpoint/mod.rs should dispatch provider key management directly to provider::endpoint_keys"
    );

    assert!(
        endpoint_mod.contains(
            "endpoints_admin::maybe_build_local_admin_endpoints_routes_response"
        ),
        "handlers/admin/endpoint/mod.rs should dispatch provider endpoint CRUD directly to provider::endpoints_admin"
    );
    for path in [
        "apps/aether-gateway/src/handlers/admin/endpoint/keys.rs",
        "apps/aether-gateway/src/handlers/admin/endpoint/routes.rs",
    ] {
        assert!(
            !workspace_file_exists(path),
            "{path} should be deleted once endpoint root dispatches directly to provider-owned handlers"
        );
    }
}

#[test]
fn admin_model_root_owns_model_catalog_routes() {
    let model_mod = read_workspace_file("apps/aether-gateway/src/handlers/admin/model/mod.rs");
    assert!(
        model_mod.contains("mod catalog_routes;"),
        "handlers/admin/model/mod.rs should register catalog_routes owner"
    );
    assert!(
        model_mod.contains(
            "pub(crate) use self::catalog_routes::maybe_build_local_admin_model_catalog_response;"
        ),
        "handlers/admin/model/mod.rs should expose model catalog route seam"
    );

    let model_catalog_routes =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/model/catalog_routes.rs");
    for pattern in [
        "build_admin_model_catalog_payload",
        "read_admin_external_models_cache",
        "clear_admin_external_models_cache",
        "ADMIN_MODEL_CATALOG_DATA_UNAVAILABLE_DETAIL",
    ] {
        assert!(
            model_catalog_routes.contains(pattern),
            "handlers/admin/model/catalog_routes.rs should own {pattern}"
        );
    }
}

#[test]
fn admin_system_owns_admin_module_helpers() {
    let public_modules_helpers =
        read_workspace_file("apps/aether-gateway/src/handlers/public/system_modules_helpers.rs");
    for pattern in [
        "admin_module_by_name",
        "admin_module_name_from_enabled_path",
        "admin_module_name_from_status_path",
        "build_admin_module_runtime_state",
        "build_admin_module_status_payload",
        "build_admin_module_validation_result",
        "build_admin_modules_status_payload",
        "AdminSetModuleEnabledRequest",
    ] {
        assert!(
            !public_modules_helpers.contains(pattern),
            "handlers/public/system_modules_helpers.rs should not re-export admin module helper {pattern}"
        );
    }

    let public_modules = read_workspace_file(
        "apps/aether-gateway/src/handlers/public/system_modules_helpers/modules.rs",
    );
    for pattern in [
        "pub(crate) struct AdminModuleDefinition",
        "pub(crate) struct AdminSetModuleEnabledRequest",
        "pub(crate) struct AdminModuleRuntimeState",
        "pub(crate) fn admin_module_by_name",
        "pub(crate) async fn build_admin_module_runtime_state",
        "pub(crate) fn build_admin_module_validation_result",
        "pub(crate) async fn build_admin_module_status_payload",
        "pub(crate) async fn build_admin_modules_status_payload",
        "pub(crate) fn admin_module_name_from_status_path",
        "pub(crate) fn admin_module_name_from_enabled_path",
    ] {
        assert!(
            !public_modules.contains(pattern),
            "handlers/public/system_modules_helpers/modules.rs should not own admin module helper {pattern}"
        );
    }

    let system_shared_modules =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/system/shared/modules.rs");
    for pattern in [
        "pub(crate) struct AdminModuleDefinition",
        "pub(crate) struct AdminSetModuleEnabledRequest",
        "pub(crate) struct AdminModuleRuntimeState",
        "pub(crate) fn admin_module_by_name",
        "pub(crate) async fn build_admin_module_runtime_state",
        "pub(crate) fn build_admin_module_validation_result",
        "pub(crate) async fn build_admin_module_status_payload",
        "pub(crate) async fn build_admin_modules_status_payload",
        "pub(crate) fn admin_module_name_from_status_path",
        "pub(crate) fn admin_module_name_from_enabled_path",
    ] {
        assert!(
            system_shared_modules.contains(pattern),
            "handlers/admin/system/shared/modules.rs should own {pattern}"
        );
    }
}

#[test]
fn admin_system_owns_system_route_helpers() {
    let public_system_helpers =
        read_workspace_file("apps/aether-gateway/src/handlers/public/system_modules_helpers.rs");
    for pattern in [
        "current_aether_version",
        "build_admin_system_check_update_payload",
        "build_admin_system_stats_payload",
        "build_admin_system_settings_payload",
        "apply_admin_system_settings_update",
        "build_admin_api_formats_payload",
        "build_admin_system_config_export_payload",
        "build_admin_system_users_export_payload",
        "build_admin_system_configs_payload",
        "build_admin_system_config_detail_payload",
        "apply_admin_system_config_update",
        "delete_admin_system_config",
        "serialize_admin_system_users_export_wallet",
        "module_available_from_env",
        "system_config_bool",
        "system_config_string",
        "read_admin_email_template_payload",
        "escape_admin_email_template_html",
        "render_admin_email_template_html",
    ] {
        assert!(
            !public_system_helpers.contains(pattern),
            "handlers/public/system_modules_helpers.rs should not re-export admin system helper {pattern}"
        );
    }

    let public_mod = read_workspace_file("apps/aether-gateway/src/handlers/public/mod.rs");
    for pattern in [
        "current_aether_version",
        "build_admin_system_check_update_payload",
        "build_admin_system_stats_payload",
        "build_admin_system_settings_payload",
        "apply_admin_system_settings_update",
        "build_admin_api_formats_payload",
        "build_admin_system_config_export_payload",
        "build_admin_system_users_export_payload",
        "build_admin_system_configs_payload",
        "build_admin_system_config_detail_payload",
        "apply_admin_system_config_update",
        "delete_admin_system_config",
        "serialize_admin_system_users_export_wallet",
        "module_available_from_env",
        "system_config_bool",
        "system_config_string",
        "read_admin_email_template_payload",
        "escape_admin_email_template_html",
        "render_admin_email_template_html",
    ] {
        assert!(
            !public_mod.contains(pattern),
            "handlers/public/mod.rs should not re-export admin system helper {pattern}"
        );
    }

    let public_system_file = read_workspace_file(
        "apps/aether-gateway/src/handlers/public/system_modules_helpers/system.rs",
    );
    for pattern in [
        "pub(crate) fn current_aether_version",
        "pub(crate) fn build_admin_system_check_update_payload",
        "pub(crate) async fn build_admin_system_stats_payload",
        "pub(crate) async fn build_admin_system_settings_payload",
        "pub(crate) async fn build_admin_system_config_export_payload",
        "pub(crate) async fn build_admin_system_users_export_payload",
        "pub(crate) fn build_admin_system_configs_payload",
        "pub(crate) async fn build_admin_system_config_detail_payload",
        "pub(crate) async fn apply_admin_system_config_update",
        "pub(crate) async fn delete_admin_system_config",
        "pub(crate) fn serialize_admin_system_users_export_wallet",
        "pub(crate) fn module_available_from_env",
        "pub(crate) fn system_config_bool",
        "pub(crate) fn system_config_string",
        "pub(crate) async fn read_admin_email_template_payload",
        "pub(crate) fn escape_admin_email_template_html",
        "pub(crate) fn render_admin_email_template_html",
    ] {
        assert!(
            !public_system_file.contains(pattern),
            "handlers/public/system_modules_helpers/system.rs should not own shared/admin system helper {pattern}"
        );
    }

    let shared_mod = read_workspace_file("apps/aether-gateway/src/handlers/shared/mod.rs");
    for pattern in [
        "mod email_templates;",
        "mod system_config_values;",
        "pub(crate) use self::email_templates::{",
        "pub(crate) use self::system_config_values::{",
    ] {
        assert!(
            shared_mod.contains(pattern),
            "handlers/shared/mod.rs should wire shared system helper owner {pattern}"
        );
    }

    let shared_system_config_values =
        read_workspace_file("apps/aether-gateway/src/handlers/shared/system_config_values.rs");
    for pattern in [
        "pub(crate) fn module_available_from_env",
        "pub(crate) fn system_config_bool",
        "pub(crate) fn system_config_string",
    ] {
        assert!(
            shared_system_config_values.contains(pattern),
            "handlers/shared/system_config_values.rs should own {pattern}"
        );
    }

    let shared_email_templates =
        read_workspace_file("apps/aether-gateway/src/handlers/shared/email_templates.rs");
    for pattern in [
        "pub(crate) fn admin_email_template_definition",
        "pub(crate) fn admin_email_template_subject_key",
        "pub(crate) fn admin_email_template_html_key",
        "pub(crate) async fn read_admin_email_template_payload",
        "pub(crate) fn escape_admin_email_template_html",
        "pub(crate) fn render_admin_email_template_html",
    ] {
        assert!(
            shared_email_templates.contains(pattern),
            "handlers/shared/email_templates.rs should own {pattern}"
        );
    }

    let system_shared_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/system/shared/mod.rs");
    for pattern in [
        "pub(crate) mod configs;",
        "pub(crate) mod email_templates;",
        "pub(crate) mod modules;",
        "pub(crate) mod paths;",
        "pub(crate) mod settings;",
    ] {
        assert!(
            system_shared_mod.contains(pattern),
            "handlers/admin/system/shared/mod.rs should wire system helper owner {pattern}"
        );
    }
    for forbidden in [
        "pub(crate) use self::configs::*;",
        "pub(crate) use self::email_templates::{",
        "pub(crate) use self::modules::*;",
        "pub(crate) use self::paths::*;",
        "pub(crate) use self::settings::*;",
    ] {
        assert!(
            !system_shared_mod.contains(forbidden),
            "handlers/admin/system/shared/mod.rs should not remain a re-export hub for {forbidden}"
        );
    }

    assert!(
        !workspace_file_exists("apps/aether-gateway/src/handlers/admin/system/shared/system.rs"),
        "handlers/admin/system/shared/system.rs should be replaced by email_templates.rs"
    );

    let system_shared_settings =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/system/shared/settings.rs");
    for pattern in [
        "pub(crate) fn current_aether_version",
        "pub(crate) fn build_admin_system_check_update_payload",
        "pub(crate) async fn build_admin_system_stats_payload",
        "pub(crate) async fn build_admin_system_settings_payload",
        "pub(crate) async fn apply_admin_system_settings_update",
        "pub(crate) fn build_admin_api_formats_payload",
    ] {
        assert!(
            system_shared_settings.contains(pattern),
            "handlers/admin/system/shared/settings.rs should own {pattern}"
        );
    }

    let system_shared_configs =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/system/shared/configs.rs");
    for pattern in [
        "pub(crate) async fn build_admin_system_config_export_payload",
        "pub(crate) fn serialize_admin_system_users_export_wallet",
        "pub(crate) async fn build_admin_system_users_export_payload",
        "pub(crate) fn build_admin_system_configs_payload",
        "pub(crate) async fn build_admin_system_config_detail_payload",
        "pub(crate) async fn apply_admin_system_config_update",
        "pub(crate) async fn delete_admin_system_config",
    ] {
        assert!(
            system_shared_configs.contains(pattern),
            "handlers/admin/system/shared/configs.rs should own {pattern}"
        );
    }

    let system_shared_email_templates = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/system/shared/email_templates.rs",
    );
    for pattern in [
        "pub(crate) async fn build_admin_email_templates_payload",
        "pub(crate) async fn build_admin_email_template_payload",
        "pub(crate) async fn apply_admin_email_template_update",
        "pub(crate) async fn preview_admin_email_template",
        "pub(crate) async fn reset_admin_email_template",
    ] {
        assert!(
            system_shared_email_templates.contains(pattern),
            "handlers/admin/system/shared/email_templates.rs should own {pattern}"
        );
    }
}
