use super::*;

#[test]
fn admin_provider_root_stays_thin() {
    let provider_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/mod.rs");
    for pattern in [
        "pub(crate) mod endpoint_keys;",
        "pub(crate) mod endpoints_admin;",
        "pub(crate) mod oauth;",
        "pub(crate) mod ops;",
        "pub(crate) mod pool;",
        "pub(crate) mod pool_admin;",
        "pub(crate) mod shared;",
        "pub(crate) mod write;",
    ] {
        assert!(
            provider_mod.contains(pattern),
            "handlers/admin/provider/mod.rs should expose provider subdomain module {pattern}"
        );
    }

    for pattern in [
        "pub(crate) use self::oauth::{",
        "pub(crate) use self::ops::{",
        "pub(crate) use self::pool::{",
        "pub(crate) use self::endpoints_admin::{",
        "pub(crate) use self::pool_admin::{",
        "pub(crate) use self::write::{",
        "build_proxy_error_response",
        "build_internal_control_error_response",
        "admin_provider_ops_local_action_response",
        "admin_provider_pool_config",
        "build_admin_create_provider_key_record",
        "build_admin_export_key_payload",
        "normalize_provider_billing_type",
        "parse_optional_rfc3339_unix_secs",
    ] {
        assert!(
            !provider_mod.contains(pattern),
            "handlers/admin/provider/mod.rs should not act as internal helper export hub for {pattern}"
        );
    }

    for pattern in [
        "pub(crate) use self::crud::maybe_build_local_admin_providers_response;",
        "pub(super) use self::models::maybe_build_local_admin_provider_models_response;",
        "pub(crate) use self::oauth::maybe_build_local_admin_provider_oauth_response;",
        "pub(super) use self::ops::maybe_build_local_admin_provider_ops_response;",
        "pub(super) use self::query::maybe_build_local_admin_provider_query_response;",
        "pub(super) use self::strategy::maybe_build_local_admin_provider_strategy_response;",
    ] {
        assert!(
            provider_mod.contains(pattern),
            "handlers/admin/provider/mod.rs should keep only the minimal route seam {pattern}"
        );
    }

    let ops_mod = read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/ops/mod.rs");
    assert!(
        !ops_mod.contains("pub(crate) use self::providers::admin_provider_ops_local_action_response;"),
        "handlers/admin/provider/ops/mod.rs should not re-export admin_provider_ops_local_action_response"
    );

    let oauth_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/oauth/mod.rs");
    for pattern in [
        "pub(crate) mod duplicates;",
        "pub(crate) mod errors;",
        "pub(crate) mod provisioning;",
        "pub(crate) mod quota;",
        "pub(crate) mod runtime;",
        "pub(crate) mod state;",
    ] {
        assert!(
            oauth_mod.contains(pattern),
            "handlers/admin/provider/oauth/mod.rs should expose explicit oauth owner {pattern}"
        );
    }
    for pattern in [
        "pub(crate) use self::quota as provider_oauth_quota;",
        "pub(crate) use self::refresh as provider_oauth_refresh;",
        "pub(crate) use self::state as provider_oauth_state;",
        "pub(crate) use self::errors::{",
        "pub(crate) use self::duplicates::{",
        "pub(crate) use self::provisioning::{",
        "pub(crate) use self::runtime::{",
        "pub(crate) mod refresh;",
    ] {
        assert!(
            !oauth_mod.contains(pattern),
            "handlers/admin/provider/oauth/mod.rs should not alias re-export {pattern}"
        );
    }
    assert!(
        oauth_mod.contains(
            "pub(crate) use self::dispatch::maybe_build_local_admin_provider_oauth_response;"
        ),
        "handlers/admin/provider/oauth/mod.rs should continue exposing the dispatch entry seam"
    );
}

#[test]
fn admin_provider_oauth_complete_dispatch_remains_thin() {
    let dispatch_mod = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/mod.rs",
    );
    for pattern in [
        "mod complete;",
        "complete::handle_admin_provider_oauth_complete_key(",
        "complete::handle_admin_provider_oauth_complete_provider(",
    ] {
        assert!(
            dispatch_mod.contains(pattern),
            "handlers/admin/provider/oauth/dispatch/mod.rs should keep complete entry seam {pattern}"
        );
    }
}

#[test]
fn admin_provider_oauth_complete_helpers_are_split() {
    let complete_mod = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/complete/mod.rs",
    );
    for pattern in ["mod key;", "mod provider;", "mod shared;"] {
        assert!(
            complete_mod.contains(pattern),
            "handlers/admin/provider/oauth/dispatch/complete/mod.rs should register specific helper {pattern}"
        );
    }
    let shared = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/complete/shared.rs",
    );
    for pattern in [
        "pub(super) fn parse_admin_provider_oauth_callback_url(",
        "pub(super) fn extract_admin_provider_oauth_code(",
        "pub(super) fn extract_admin_provider_oauth_state(",
    ] {
        assert!(
            shared.contains(pattern),
            "handlers/admin/provider/oauth/dispatch/complete/shared.rs should own shared helper {pattern}"
        );
    }
    let key = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/complete/key.rs",
    );
    assert!(
        key.contains("pub(super) async fn handle_admin_provider_oauth_complete_key("),
        "handlers/admin/provider/oauth/dispatch/complete/key.rs should own key completion logic"
    );
    let provider = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/complete/provider.rs",
    );
    assert!(
        provider.contains("pub(super) async fn handle_admin_provider_oauth_complete_provider("),
        "handlers/admin/provider/oauth/dispatch/complete/provider.rs should own provider completion logic"
    );
    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/complete.rs"
        ),
        "dispatch/complete.rs should be removed once completion logic is split"
    );
}

#[test]
fn admin_provider_endpoints_admin_mod_uses_specific_route_owners() {
    let endpoints_admin_mod = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/endpoints_admin/mod.rs",
    );
    for pattern in [
        "mod create;",
        "mod defaults;",
        "mod delete;",
        "mod detail;",
        "mod list;",
        "mod reads;",
        "mod update;",
        "create::maybe_handle(state, request_context, request_body)",
        "update::maybe_handle(state, request_context, request_body)",
        "delete::maybe_handle(state, request_context, request_body)",
        "list::maybe_handle(state, request_context, request_body)",
        "detail::maybe_handle(state, request_context, request_body)",
        "defaults::maybe_handle(state, request_context, request_body)",
    ] {
        assert!(
            endpoints_admin_mod.contains(pattern),
            "handlers/admin/provider/endpoints_admin/mod.rs should dispatch through explicit route owner {pattern}"
        );
    }

    for forbidden in [
        "mod builders;",
        "mod read_routes;",
        "mod write_routes;",
        "super::builders::",
        "read_routes::maybe_build_local_admin_endpoints_read_response",
        "write_routes::maybe_build_local_admin_endpoints_write_response",
    ] {
        assert!(
            !endpoints_admin_mod.contains(forbidden),
            "handlers/admin/provider/endpoints_admin/mod.rs should not keep route bus seam {forbidden}"
        );
    }

    for path in [
        "apps/aether-gateway/src/handlers/admin/provider/endpoints_admin/create.rs",
        "apps/aether-gateway/src/handlers/admin/provider/endpoints_admin/update.rs",
        "apps/aether-gateway/src/handlers/admin/provider/endpoints_admin/delete.rs",
        "apps/aether-gateway/src/handlers/admin/provider/endpoints_admin/list.rs",
        "apps/aether-gateway/src/handlers/admin/provider/endpoints_admin/detail.rs",
        "apps/aether-gateway/src/handlers/admin/provider/endpoints_admin/defaults.rs",
        "apps/aether-gateway/src/handlers/admin/provider/endpoints_admin/reads.rs",
    ] {
        assert!(
            workspace_file_exists(path),
            "{path} should exist once endpoints_admin dispatches through specific route owners"
        );
    }

    for path in [
        "apps/aether-gateway/src/handlers/admin/provider/endpoints_admin/builders.rs",
        "apps/aether-gateway/src/handlers/admin/provider/endpoints_admin/read_routes.rs",
        "apps/aether-gateway/src/handlers/admin/provider/endpoints_admin/write_routes.rs",
        "apps/aether-gateway/src/handlers/admin/provider/endpoints_admin/writes.rs",
    ] {
        assert!(
            !workspace_file_exists(path),
            "{path} should be deleted once endpoints_admin stops routing through read/write buses"
        );
    }

    let endpoints_admin_reads = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/endpoints_admin/reads.rs",
    );
    for pattern in [
        "pub(crate) async fn build_admin_provider_endpoints_payload(",
        "pub(crate) async fn build_admin_endpoint_payload(",
    ] {
        assert!(
            endpoints_admin_reads.contains(pattern),
            "handlers/admin/provider/endpoints_admin/reads.rs should own {pattern}"
        );
    }

    let request_provider_builders =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/request/provider/builders.rs");
    for pattern in [
        "pub(crate) async fn build_admin_create_provider_endpoint_record(",
        "pub(crate) async fn build_admin_update_provider_endpoint_record(",
    ] {
        assert!(
            request_provider_builders.contains(pattern),
            "handlers/admin/request/provider/builders.rs should own {pattern}"
        );
    }

    for (path, expected) in [
        (
            "apps/aether-gateway/src/handlers/admin/provider/endpoints_admin/create.rs",
            ".build_admin_create_provider_endpoint_record(",
        ),
        (
            "apps/aether-gateway/src/handlers/admin/provider/endpoints_admin/update.rs",
            ".build_admin_update_provider_endpoint_record(",
        ),
        (
            "apps/aether-gateway/src/handlers/admin/provider/endpoints_admin/list.rs",
            "use super::reads::build_admin_provider_endpoints_payload;",
        ),
        (
            "apps/aether-gateway/src/handlers/admin/provider/endpoints_admin/detail.rs",
            "use super::reads::build_admin_endpoint_payload;",
        ),
    ] {
        let contents = read_workspace_file(path);
        assert!(
            contents.contains(expected),
            "{path} should import or delegate through explicit endpoint owner {expected}"
        );
        assert!(
            !contents.contains("super::builders::"),
            "{path} should not depend on the removed endpoints_admin::builders hub"
        );
        assert!(
            !contents.contains("super::writes::"),
            "{path} should not depend on the removed endpoints_admin::writes owner"
        );
    }
}

#[test]
fn admin_provider_ops_routes_directoryized() {
    let routes_mod_path =
        "apps/aether-gateway/src/handlers/admin/provider/ops/providers/routes/mod.rs";
    assert!(
        workspace_file_exists(routes_mod_path),
        "handlers/admin/provider/ops/providers/routes/mod.rs must exist after directoryizing provider ops routes"
    );
    let routes_mod = read_workspace_file(routes_mod_path);

    for pattern in [
        "mod batch;",
        "mod config;",
        "mod verify;",
        "mod connect;",
        "mod actions;",
        "mod read;",
    ] {
        assert!(
            routes_mod.contains(pattern),
            "handlers/admin/provider/ops/providers/routes/mod.rs should register {pattern}"
        );
    }

    for pattern in [
        "batch::handle_admin_provider_ops_batch_balance(",
        "config::handle_admin_provider_ops_save_config(",
        "verify::handle_admin_provider_ops_verify(",
        "connect::handle_admin_provider_ops_connect(",
        "actions::handle_admin_provider_ops_action(",
        "read::handle_admin_provider_ops_read(",
    ] {
        assert!(
            routes_mod.contains(pattern),
            "handlers/admin/provider/ops/providers/routes/mod.rs should delegate {pattern} to the owner module"
        );
    }

    assert!(
        routes_mod.contains("pub(crate) async fn maybe_build_local_admin_provider_ops_providers_response("),
        "handlers/admin/provider/ops/providers/routes/mod.rs should keep the provider ops entry seam"
    );
    for forbidden in [
        "admin_provider_ops_local_action_response(",
        "build_admin_provider_ops_saved_config_value(",
        "admin_provider_ops_local_verify_response(",
        "build_admin_provider_ops_status_payload(",
        "build_admin_provider_ops_config_payload(",
    ] {
        assert!(
            !routes_mod.contains(forbidden),
            "handlers/admin/provider/ops/providers/routes/mod.rs should not own provider ops implementation {forbidden}"
        );
    }
    assert!(
        !workspace_file_exists("apps/aether-gateway/src/handlers/admin/provider/ops/providers/routes.rs"),
        "handlers/admin/provider/ops/providers/routes.rs should be removed once routes are directoryized"
    );

    for path in [
        "apps/aether-gateway/src/handlers/admin/provider/ops/providers/routes/batch.rs",
        "apps/aether-gateway/src/handlers/admin/provider/ops/providers/routes/config.rs",
        "apps/aether-gateway/src/handlers/admin/provider/ops/providers/routes/verify.rs",
        "apps/aether-gateway/src/handlers/admin/provider/ops/providers/routes/connect.rs",
        "apps/aether-gateway/src/handlers/admin/provider/ops/providers/routes/actions.rs",
        "apps/aether-gateway/src/handlers/admin/provider/ops/providers/routes/read.rs",
    ] {
        assert!(
            workspace_file_exists(path),
            "{path} should exist after directoryizing provider ops routes"
        );
    }
}

#[test]
fn admin_provider_ops_route_owners_stay_explicit() {
    let batch = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/ops/providers/routes/batch.rs",
    );
    assert!(
        batch.contains("super::super::actions::admin_provider_ops_local_action_response"),
        "batch.rs should depend directly on actions owner"
    );

    let config = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/ops/providers/routes/config.rs",
    );
    assert!(
        config.contains("super::super::config::build_admin_provider_ops_saved_config_value"),
        "config.rs should depend directly on config owner"
    );

    let verify = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/ops/providers/routes/verify.rs",
    );
    assert!(
        verify.contains("super::super::verify::admin_provider_ops_local_verify_response"),
        "verify.rs should depend directly on gateway verify runtime owner"
    );

    let connect = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/ops/providers/routes/connect.rs",
    );
    assert!(
        connect.contains("super::super::support::{"),
        "connect.rs should depend directly on support owner"
    );

    let actions = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/ops/providers/routes/actions.rs",
    );
    assert!(
        actions.contains("super::super::actions::{"),
        "actions.rs should depend directly on actions owner"
    );

    let read = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/ops/providers/routes/read.rs",
    );
    assert!(
        read.contains("super::super::config::{"),
        "read.rs should depend directly on config payload owner"
    );
}

#[test]
fn admin_provider_ops_architecture_registry_uses_pure_owner() {
    let architectures =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/ops/architectures.rs");
    for pattern in [
        "use aether_admin::provider::ops::{get_architecture, list_architectures};",
        "list_architectures(false)",
        "get_architecture(architecture_id)",
    ] {
        assert!(
            architectures.contains(pattern),
            "handlers/admin/provider/ops/architectures.rs should delegate architecture registry to pure owner {pattern}"
        );
    }
    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/handlers/admin/provider/ops/architectures.all.json"
        ),
        "handlers/admin/provider/ops/architectures.all.json should be removed after moving architecture registry into aether-admin"
    );
}

#[test]
fn admin_provider_summary_mod_stays_thin() {
    let summary_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/summary/mod.rs");
    for pattern in [
        "mod aggregates;",
        "mod health;",
        "mod list;",
        "mod value;",
        "pub(crate) use self::aggregates::{",
        "pub(crate) use self::health::build_admin_provider_health_monitor_payload;",
        "pub(crate) use self::list::build_admin_providers_payload;",
        "pub(crate) use self::value::build_admin_provider_summary_value;",
    ] {
        assert!(
            summary_mod.contains(pattern),
            "handlers/admin/provider/summary/mod.rs should keep explicit summary boundary {pattern}"
        );
    }
    for forbidden in [
        "pub(crate) async fn build_admin_providers_payload(",
        "pub(crate) async fn build_admin_provider_summary_payload(",
        "pub(crate) async fn build_admin_providers_summary_payload(",
        "pub(crate) async fn build_admin_provider_health_monitor_payload(",
        "pub(crate) fn build_admin_provider_summary_value(",
    ] {
        assert!(
            !summary_mod.contains(forbidden),
            "handlers/admin/provider/summary/mod.rs should not own concrete summary implementation {forbidden}"
        );
    }

    assert!(
        !workspace_file_exists("apps/aether-gateway/src/handlers/admin/provider/summary.rs"),
        "handlers/admin/provider/summary.rs should be removed once provider summary is directoryized"
    );

    for (path, expected) in [
        (
            "apps/aether-gateway/src/handlers/admin/provider/summary/list.rs",
            "pub(crate) async fn build_admin_providers_payload(",
        ),
        (
            "apps/aether-gateway/src/handlers/admin/provider/summary/value.rs",
            "pub(crate) fn build_admin_provider_summary_value(",
        ),
        (
            "apps/aether-gateway/src/handlers/admin/provider/summary/aggregates.rs",
            "pub(crate) async fn build_admin_provider_summary_payload(",
        ),
        (
            "apps/aether-gateway/src/handlers/admin/provider/summary/health.rs",
            "pub(crate) async fn build_admin_provider_health_monitor_payload(",
        ),
    ] {
        let contents = read_workspace_file(path);
        assert!(contents.contains(expected), "{path} should own {expected}");
    }
}

#[test]
fn admin_provider_strategy_uses_shared_billing_normalizers() {
    let strategy_builders =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/strategy/builders.rs");
    assert!(
        !strategy_builders.contains("use super::super::write::{"),
        "handlers/admin/provider/strategy/builders.rs should not borrow billing/time normalizers from provider::write"
    );
    assert!(
        strategy_builders.contains("crate::handlers::admin::provider::shared::support::{"),
        "handlers/admin/provider/strategy/builders.rs should import shared provider normalizers from provider::shared::support"
    );

    let provider_shared_support =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/shared/support.rs");
    for pattern in [
        "pub(crate) fn normalize_provider_billing_type(",
        "pub(crate) fn parse_optional_rfc3339_unix_secs(",
    ] {
        assert!(
            provider_shared_support.contains(pattern),
            "handlers/admin/provider/shared/support.rs should own provider-wide billing/time normalizer {pattern}"
        );
    }
}

#[test]
fn admin_provider_shared_paths_are_split_by_domain() {
    assert!(
        !workspace_file_exists("apps/aether-gateway/src/handlers/admin/provider/shared/paths.rs"),
        "handlers/admin/provider/shared/paths.rs should be replaced by paths/ directory owners"
    );

    let provider_shared_paths_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/shared/paths/mod.rs");
    for pattern in [
        "mod crud;",
        "mod endpoint_keys;",
        "mod oauth;",
        "mod ops;",
        "mod strategy;",
        "pub(crate) use self::crud::{",
        "pub(crate) use self::endpoint_keys::{",
        "pub(crate) use self::oauth::{",
        "pub(crate) use self::ops::{",
        "pub(crate) use self::strategy::{",
    ] {
        assert!(
            provider_shared_paths_mod.contains(pattern),
            "handlers/admin/provider/shared/paths/mod.rs should register split owner {pattern}"
        );
    }

    for path in [
        "apps/aether-gateway/src/handlers/admin/provider/shared/paths/crud.rs",
        "apps/aether-gateway/src/handlers/admin/provider/shared/paths/endpoint_keys.rs",
        "apps/aether-gateway/src/handlers/admin/provider/shared/paths/oauth.rs",
        "apps/aether-gateway/src/handlers/admin/provider/shared/paths/ops.rs",
        "apps/aether-gateway/src/handlers/admin/provider/shared/paths/strategy.rs",
    ] {
        assert!(
            workspace_file_exists(path),
            "{path} should exist once provider shared path extractors are split by domain"
        );
    }
}

#[test]
fn admin_provider_query_and_strategy_use_specific_local_owners() {
    let query_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/query/mod.rs");
    for pattern in ["mod payload;", "mod response;", "mod routes;"] {
        assert!(
            query_mod.contains(pattern),
            "handlers/admin/provider/query/mod.rs should register specific local owner {pattern}"
        );
    }
    assert!(
        !query_mod.contains("mod shared;"),
        "handlers/admin/provider/query/mod.rs should not retain a generic shared module"
    );

    for path in [
        "apps/aether-gateway/src/handlers/admin/provider/query/models.rs",
        "apps/aether-gateway/src/handlers/admin/provider/query/routes.rs",
    ] {
        let contents = read_workspace_file(path);
        assert!(
            !contents.contains("super::shared::{"),
            "{path} should not depend on a generic query::shared hub"
        );
    }

    assert!(
        workspace_file_exists("apps/aether-gateway/src/handlers/admin/provider/query/payload.rs"),
        "handlers/admin/provider/query/payload.rs should own provider query parsing and extractors"
    );
    assert!(
        workspace_file_exists("apps/aether-gateway/src/handlers/admin/provider/query/response.rs"),
        "handlers/admin/provider/query/response.rs should own provider query response helpers"
    );
    let query_routes =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/query/routes.rs");
    assert!(
        query_routes.contains("state\n        .maybe_build_admin_provider_query_route_response(")
            || query_routes.contains("state.maybe_build_admin_provider_query_route_response("),
        "handlers/admin/provider/query/routes.rs should delegate to request/provider route owner"
    );

    let strategy_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/strategy/mod.rs");
    for pattern in ["mod builders;", "mod responses;", "mod routes;"] {
        assert!(
            strategy_mod.contains(pattern),
            "handlers/admin/provider/strategy/mod.rs should register specific local owner {pattern}"
        );
    }
    assert!(
        !strategy_mod.contains("mod shared;"),
        "handlers/admin/provider/strategy/mod.rs should not retain a generic shared module"
    );

    let strategy_routes =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/strategy/routes.rs");
    assert!(
        strategy_routes.contains("state\n        .maybe_build_admin_provider_strategy_route_response(")
            || strategy_routes.contains("state.maybe_build_admin_provider_strategy_route_response("),
        "handlers/admin/provider/strategy/routes.rs should delegate to request/provider route owner"
    );
    assert!(
        !strategy_routes.contains("use super::shared::{")
            && !strategy_routes.contains("use super::responses::{")
            && !strategy_routes.contains("use super::builders::{"),
        "handlers/admin/provider/strategy/routes.rs should stay as a thin bridge without local implementation imports"
    );

    let strategy_builders =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/strategy/builders.rs");
    assert!(
        !strategy_builders.contains("use super::shared::"),
        "handlers/admin/provider/strategy/builders.rs should keep provider-not-found response local"
    );

    assert!(
        workspace_file_exists("apps/aether-gateway/src/handlers/admin/provider/strategy/responses.rs"),
        "handlers/admin/provider/strategy/responses.rs should own strategy route-level shared responses"
    );
    assert!(
        !workspace_file_exists("apps/aether-gateway/src/handlers/admin/provider/strategy/shared.rs"),
        "handlers/admin/provider/strategy/shared.rs should be removed once the local shared hub is narrowed"
    );
}

#[test]
fn admin_provider_crud_mod_stays_thin() {
    let crud_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/crud/mod.rs");
    for pattern in [
        "mod delete_task;",
        "mod pool;",
        "mod reads;",
        "mod responses;",
        "mod routes;",
        "mod writes;",
        "pub(crate) use self::routes::maybe_build_local_admin_providers_response;",
    ] {
        assert!(
            crud_mod.contains(pattern),
            "handlers/admin/provider/crud/mod.rs should register local owner {pattern}"
        );
    }
    for forbidden in [
        "mod shared;",
        "use shared::*;",
        "build_admin_create_provider_record(",
        "build_admin_provider_health_monitor_payload(",
        "run_admin_provider_delete_task(",
        "clear_admin_provider_pool_cooldown(",
    ] {
        assert!(
            !crud_mod.contains(forbidden),
            "handlers/admin/provider/crud/mod.rs should not retain generic shared glue {forbidden}"
        );
    }

    let crud_routes =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/crud/routes.rs");
    {
        let pattern = "maybe_build_admin_provider_crud_route_response(";
        assert!(
            crud_routes.contains(pattern),
            "handlers/admin/provider/crud/routes.rs should delegate through request/provider owner {pattern}"
        );
    }
    for forbidden in [
        "use super::{delete_task, pool, reads, writes};",
        "build_admin_create_provider_record(",
        "build_admin_provider_health_monitor_payload(",
        "run_admin_provider_delete_task(",
        "clear_admin_provider_pool_cooldown(",
    ] {
        assert!(
            !crud_routes.contains(forbidden),
            "handlers/admin/provider/crud/routes.rs should not remain an implementation hub for {forbidden}"
        );
    }

    assert!(
        workspace_file_exists("apps/aether-gateway/src/handlers/admin/provider/crud/responses.rs"),
        "handlers/admin/provider/crud/responses.rs should own provider CRUD response helpers"
    );
    let request_provider_routes = read_workspace_module_tree(
        "apps/aether-gateway/src/handlers/admin/request/provider/routes/mod.rs",
    );
    for pattern in [
        "pub(crate) async fn maybe_build_admin_provider_query_route_response(",
        "pub(crate) async fn maybe_build_admin_provider_strategy_route_response(",
        "pub(crate) async fn maybe_build_admin_provider_crud_route_response(",
    ] {
        assert!(
            request_provider_routes.contains(pattern),
            "handlers/admin/request/provider/routes/mod.rs should own {pattern}"
        );
    }
    for path in [
        "apps/aether-gateway/src/handlers/admin/provider/crud/writes.rs",
        "apps/aether-gateway/src/handlers/admin/provider/crud/reads.rs",
        "apps/aether-gateway/src/handlers/admin/provider/crud/delete_task.rs",
        "apps/aether-gateway/src/handlers/admin/provider/crud/pool.rs",
    ] {
        assert!(
            workspace_file_exists(path),
            "handlers/admin/provider/crud should own split implementation file {path}"
        );
    }
    assert!(
        !workspace_file_exists("apps/aether-gateway/src/handlers/admin/provider/crud/shared.rs"),
        "handlers/admin/provider/crud/shared.rs should be removed once the local shared hub is narrowed"
    );
}

#[test]
fn admin_provider_pool_uses_config_and_runtime_owners() {
    let pool_mod = read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/pool.rs");
    for pattern in ["pub(crate) mod config;", "pub(crate) mod runtime;"] {
        assert!(
            pool_mod.contains(pattern),
            "handlers/admin/provider/pool.rs should expose explicit pool owner {pattern}"
        );
    }
    for forbidden in [
        "pub(crate) use config::admin_provider_pool_config;",
        "pub(crate) use runtime::{",
        "fn admin_provider_pool_lru_enabled(",
        "fn pool_sticky_pattern(",
    ] {
        assert!(
            !pool_mod.contains(forbidden),
            "handlers/admin/provider/pool.rs should not remain a pool helper implementation hub for {forbidden}"
        );
    }

    let pool_config =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/pool/config.rs");
    assert!(
        pool_config.contains("pub(crate) fn admin_provider_pool_config("),
        "handlers/admin/provider/pool/config.rs should own provider pool config parsing"
    );

    assert!(
        !workspace_file_exists("apps/aether-gateway/src/handlers/admin/provider/pool/runtime.rs"),
        "handlers/admin/provider/pool/runtime.rs should be replaced by runtime/ directory owners"
    );
    let pool_runtime =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/pool/runtime/mod.rs");
    for pattern in [
        "mod keys;",
        "mod mutations;",
        "mod reads;",
        "mod status;",
        "pub(crate) use self::reads::{",
        "pub(crate) use self::status::build_admin_provider_pool_status_payload;",
        "pub(crate) use self::mutations::{",
    ] {
        assert!(
            pool_runtime.contains(pattern),
            "handlers/admin/provider/pool/runtime/mod.rs should own pool runtime seam {pattern}"
        );
    }
    for path in [
        "apps/aether-gateway/src/handlers/admin/provider/pool/runtime/keys.rs",
        "apps/aether-gateway/src/handlers/admin/provider/pool/runtime/reads.rs",
        "apps/aether-gateway/src/handlers/admin/provider/pool/runtime/status.rs",
        "apps/aether-gateway/src/handlers/admin/provider/pool/runtime/mutations.rs",
    ] {
        assert!(
            workspace_file_exists(path),
            "{path} should exist once pool runtime is split into specific owners"
        );
    }

    let pool_admin_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/pool_admin/mod.rs");
    for pattern in [
        "crate::handlers::admin::provider::pool::config::admin_provider_pool_config",
        "crate::handlers::admin::provider::pool::runtime::{",
    ] {
        assert!(
            pool_admin_mod.contains(pattern),
            "handlers/admin/provider/pool_admin/mod.rs should import explicit pool owner {pattern}"
        );
    }

    let crud_reads =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/crud/reads.rs");
    assert!(
        crud_reads.contains("state")
            && crud_reads.contains(".build_admin_provider_pool_status_payload(&provider_id)")
            && crud_reads.contains(".await"),
        "handlers/admin/provider/crud/reads.rs should delegate pool-status reads through wrapped admin state capability"
    );

    let crud_pool =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/crud/pool.rs");
    assert!(
        crud_pool.contains(".clear_admin_provider_pool_cooldown(")
            && crud_pool.contains(".reset_admin_provider_pool_cost("),
        "handlers/admin/provider/crud/pool.rs should delegate pool mutations through wrapped admin state capability"
    );
}

#[test]
fn admin_provider_pool_admin_mod_stays_thin() {
    let pool_admin_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/pool_admin/mod.rs");
    for pattern in [
        "mod support;",
        "mod payloads;",
        "mod selection;",
        "#[path = \"batch_routes/action.rs\"]",
        "#[path = \"batch_routes/cleanup.rs\"]",
        "#[path = \"batch_routes/import.rs\"]",
        "#[path = \"batch_routes/shared.rs\"]",
        "#[path = \"batch_routes/task_status.rs\"]",
        "#[path = \"read_routes/keys.rs\"]",
        "#[path = \"read_routes/overview.rs\"]",
        "#[path = \"read_routes/presets.rs\"]",
        "#[path = \"read_routes/resolve_selection.rs\"]",
        "use self::support::{build_admin_pool_error_response, is_admin_pool_route};",
    ] {
        assert!(
            pool_admin_mod.contains(pattern),
            "handlers/admin/provider/pool_admin/mod.rs should keep explicit boundary {pattern}"
        );
    }
    for forbidden in [
        "const ADMIN_POOL_PROVIDER_CATALOG_READER_UNAVAILABLE_DETAIL:",
        "const ADMIN_POOL_PROVIDER_CATALOG_WRITER_UNAVAILABLE_DETAIL:",
        "const ADMIN_POOL_BANNED_KEY_CLEANUP_EMPTY_MESSAGE:",
        "struct AdminPoolResolveSelectionRequest",
        "fn parse_admin_pool_page(",
        "fn parse_admin_pool_page_size(",
        "fn parse_admin_pool_quick_selectors(",
        "fn parse_admin_pool_search(",
        "fn parse_admin_pool_status_filter(",
        "fn admin_pool_provider_id_from_path(",
        "fn is_admin_pool_route(",
        "fn build_admin_pool_error_response(",
    ] {
        assert!(
            !pool_admin_mod.contains(forbidden),
            "handlers/admin/provider/pool_admin/mod.rs should not remain a local helper hub for {forbidden}"
        );
    }

    let pool_admin_support = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/pool_admin/support.rs",
    );
    for pattern in [
        "pub(crate) const ADMIN_POOL_PROVIDER_CATALOG_READER_UNAVAILABLE_DETAIL:",
        "pub(crate) const ADMIN_POOL_PROVIDER_CATALOG_WRITER_UNAVAILABLE_DETAIL:",
        "pub(crate) const ADMIN_POOL_BANNED_KEY_CLEANUP_EMPTY_MESSAGE:",
        "pub(crate) use aether_admin::provider::pool::AdminPoolResolveSelectionRequest;",
        "pub(crate) fn parse_admin_pool_page(",
        "pub(crate) fn parse_admin_pool_page_size(",
        "pub(crate) fn parse_admin_pool_quick_selectors(",
        "pub(crate) fn parse_admin_pool_search(",
        "pub(crate) fn parse_admin_pool_status_filter(",
        "pub(crate) fn admin_pool_provider_id_from_path(",
        "pub(crate) fn is_admin_pool_route(",
        "pub(crate) fn build_admin_pool_error_response(",
    ] {
        assert!(
            pool_admin_support.contains(pattern),
            "handlers/admin/provider/pool_admin/support.rs should own {pattern}"
        );
    }

    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/handlers/admin/provider/pool_admin/read_routes/mod.rs"
        ),
        "handlers/admin/provider/pool_admin/read_routes/mod.rs should be removed once root pool_admin mod dispatches directly to read owners"
    );
    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/handlers/admin/provider/pool_admin/batch_routes/mod.rs"
        ),
        "handlers/admin/provider/pool_admin/batch_routes/mod.rs should be removed once root pool_admin mod dispatches directly to batch owners"
    );
    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/handlers/admin/provider/pool_admin/read_routes.rs"
        ),
        "handlers/admin/provider/pool_admin/read_routes.rs should be removed once read routes are directoryized"
    );
    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/handlers/admin/provider/pool_admin/batch_routes.rs"
        ),
        "handlers/admin/provider/pool_admin/batch_routes.rs should be removed once batch routes are directoryized"
    );
}

#[test]
fn admin_provider_write_uses_specific_local_owners() {
    let write_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/write/mod.rs");
    for pattern in [
        "pub(crate) mod keys;",
        "pub(crate) mod normalize;",
        "pub(crate) mod provider;",
        "pub(crate) mod reveal;",
    ] {
        assert!(
            write_mod.contains(pattern),
            "handlers/admin/provider/write/mod.rs should expose explicit write owner {pattern}"
        );
    }
    for forbidden in [
        "pub(crate) use self::keys::{",
        "pub(crate) use self::provider::{",
        "pub(crate) use self::reveal::{",
        "pub(crate) fn normalize_provider_type_input(",
        "pub(crate) fn normalize_auth_type(",
        "pub(crate) fn validate_vertex_api_formats(",
    ] {
        assert!(
            !write_mod.contains(forbidden),
            "handlers/admin/provider/write/mod.rs should not remain a write helper export hub for {forbidden}"
        );
    }

    let write_normalize =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/write/normalize.rs");
    for pattern in [
        "pub(crate) fn normalize_provider_type_input(",
        "pub(crate) fn normalize_auth_type(",
        "pub(crate) fn validate_vertex_api_formats(",
    ] {
        assert!(
            write_normalize.contains(pattern),
            "handlers/admin/provider/write/normalize.rs should own write normalization helper {pattern}"
        );
    }

    assert!(
        !workspace_file_exists("apps/aether-gateway/src/handlers/admin/provider/write/keys.rs"),
        "handlers/admin/provider/write/keys.rs should be removed once keys owner is directoryized"
    );
    let write_keys =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/write/keys/mod.rs");
    for pattern in [
        "mod create;",
        "mod payload;",
        "mod update;",
        "create::build_admin_create_provider_key_record",
        "payload::build_admin_provider_keys_payload",
        "update::build_admin_update_provider_key_record",
    ] {
        assert!(
            write_keys.contains(pattern),
            "handlers/admin/provider/write/keys/mod.rs should expose explicit keys owner {pattern}"
        );
    }

    let write_provider =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/write/provider.rs");
    for pattern in [
        "mod create;",
        "mod endpoint;",
        "mod template;",
        "mod update;",
        "pub(crate) use self::create::build_admin_create_provider_record;",
        "pub(crate) use self::endpoint::build_admin_fixed_provider_endpoint_record;",
        "pub(crate) use self::template::{",
        "pub(crate) use self::update::build_admin_update_provider_record;",
    ] {
        assert!(
            write_provider.contains(pattern),
            "handlers/admin/provider/write/provider.rs should expose explicit provider-write owner {pattern}"
        );
    }

    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/handlers/admin/provider/write/keys/records.rs"
        ),
        "handlers/admin/provider/write/keys/records.rs should be removed once key-write owners are split"
    );

    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/handlers/admin/provider/write/provider/records.rs"
        ),
        "handlers/admin/provider/write/provider/records.rs should be removed once provider-write owners are split"
    );

    let write_provider_create = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/write/provider/create.rs",
    );
    for pattern in [
        "pub(crate) async fn build_admin_create_provider_record(",
        "crate::handlers::admin::provider::write::normalize::normalize_provider_type_input;",
        "crate::handlers::admin::shared::normalize_json_object;",
    ] {
        assert!(
            write_provider_create.contains(pattern),
            "handlers/admin/provider/write/provider/create.rs should own {pattern}"
        );
    }

    let write_provider_update = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/write/provider/update.rs",
    );
    for pattern in [
        "pub(crate) async fn build_admin_update_provider_record(",
        "crate::handlers::admin::provider::write::normalize::normalize_provider_type_input;",
        "crate::handlers::admin::shared::normalize_json_object;",
    ] {
        assert!(
            write_provider_update.contains(pattern),
            "handlers/admin/provider/write/provider/update.rs should own {pattern}"
        );
    }

    let write_provider_endpoint = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/write/provider/endpoint.rs",
    );
    for pattern in [
        "pub(crate) fn build_admin_fixed_provider_endpoint_record(",
        "admin_endpoint_signature_parts(",
        "normalize_admin_base_url(template.base_url)?",
    ] {
        assert!(
            write_provider_endpoint.contains(pattern),
            "handlers/admin/provider/write/provider/endpoint.rs should own {pattern}"
        );
    }

    let write_provider_template = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/write/provider/template.rs",
    );
    for pattern in [
        "pub(crate) async fn reconcile_admin_fixed_provider_template_endpoints(",
        "pub(crate) fn apply_admin_fixed_provider_endpoint_template_overrides(",
        "const FIXED_PROVIDER_TEMPLATE_METADATA_KEY: &str = \"_aether_fixed_provider_template\";",
    ] {
        assert!(
            write_provider_template.contains(pattern),
            "handlers/admin/provider/write/provider/template.rs should own {pattern}"
        );
    }

    let write_key_create =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/write/keys/create.rs");
    for pattern in [
        "pub(crate) async fn build_admin_create_provider_key_record(",
        "crate::handlers::admin::provider::write::normalize::{",
        "normalize_auth_type, validate_vertex_api_formats,",
        "normalize_json_object, normalize_string_list,",
    ] {
        assert!(
            write_key_create.contains(pattern),
            "handlers/admin/provider/write/keys/create.rs should own {pattern}"
        );
    }

    let write_key_payload = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/write/keys/payload.rs",
    );
    for pattern in [
        "pub(crate) async fn build_admin_provider_keys_payload(",
        "build_admin_provider_key_response(",
    ] {
        assert!(
            write_key_payload.contains(pattern),
            "handlers/admin/provider/write/keys/payload.rs should own {pattern}"
        );
    }

    let write_key_update =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/write/keys/update.rs");
    for pattern in [
        "pub(crate) async fn build_admin_update_provider_key_record(",
        "crate::handlers::admin::provider::write::normalize::{",
        "normalize_auth_type, validate_vertex_api_formats,",
        "encrypt_catalog_secret_with_fallbacks, json_string_list,",
        "normalize_json_object, normalize_string_list,",
    ] {
        assert!(
            write_key_update.contains(pattern),
            "handlers/admin/provider/write/keys/update.rs should own {pattern}"
        );
    }

    let endpoint_keys_reads = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/endpoint_keys/reads.rs",
    );
    assert!(
        endpoint_keys_reads.contains("state.build_admin_reveal_key_payload(&key)")
            && endpoint_keys_reads.contains("state.build_admin_export_key_payload(&key).await"),
        "handlers/admin/provider/endpoint_keys/reads.rs should delegate reveal/export payload building through wrapped admin state capability"
    );
    let endpoint_keys_mutations = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/endpoint_keys/mutations/mod.rs",
    );
    for pattern in [
        "mod batch;",
        "mod create;",
        "mod delete;",
        "mod oauth_invalid;",
        "mod update;",
    ] {
        assert!(
            endpoint_keys_mutations.contains(pattern),
             "handlers/admin/provider/endpoint_keys/mutations/mod.rs should expose explicit mutation owner {pattern}"
        );
    }

    let crud_writes =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/crud/writes.rs");
    assert!(
        crud_writes.contains(".build_admin_create_provider_record(")
            && crud_writes.contains(".build_admin_update_provider_record("),
        "handlers/admin/provider/crud/writes.rs should delegate provider record builders through wrapped admin state capability"
    );
}

#[test]
fn admin_provider_ops_providers_mod_stays_thin() {
    let providers_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/ops/providers/mod.rs");
    for pattern in [
        "pub(crate) mod actions;",
        "mod config;",
        "mod routes;",
        "mod support;",
        "mod verify;",
        "pub(super) use self::routes::maybe_build_local_admin_provider_ops_providers_response;",
    ] {
        assert!(
            providers_mod.contains(pattern),
            "handlers/admin/provider/ops/providers/mod.rs should keep explicit boundary {pattern}"
        );
    }
    for pattern in [
        "pub(crate) use self::actions::admin_provider_ops_local_action_response;",
        "const ADMIN_PROVIDER_OPS_SENSITIVE_FIELDS:",
        "const ADMIN_PROVIDER_OPS_CONNECT_RUST_ONLY_MESSAGE:",
        "const ADMIN_PROVIDER_OPS_ACTION_RUST_ONLY_MESSAGE:",
        "const ADMIN_PROVIDER_OPS_VERIFY_RUST_ONLY_MESSAGE:",
        "struct AdminProviderOpsSaveConfigRequest",
        "struct AdminProviderOpsConnectRequest",
        "struct AdminProviderOpsExecuteActionRequest",
        "struct AdminProviderOpsCheckinOutcome",
    ] {
        assert!(
            !providers_mod.contains(pattern),
            "handlers/admin/provider/ops/providers/mod.rs should not keep helper/data owner {pattern}"
        );
    }

    let providers_support = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/ops/providers/support.rs",
    );
    for pattern in [
        "pub(super) const ADMIN_PROVIDER_OPS_SENSITIVE_FIELDS:",
        "pub(super) const ADMIN_PROVIDER_OPS_CONNECT_RUST_ONLY_MESSAGE:",
        "pub(super) const ADMIN_PROVIDER_OPS_ACTION_RUST_ONLY_MESSAGE:",
        "pub(super) const ADMIN_PROVIDER_OPS_VERIFY_RUST_ONLY_MESSAGE:",
        "pub(super) struct AdminProviderOpsSaveConfigRequest",
        "pub(super) struct AdminProviderOpsConnectRequest",
        "pub(super) struct AdminProviderOpsExecuteActionRequest",
        "ProviderOpsCheckinOutcome as AdminProviderOpsCheckinOutcome",
    ] {
        assert!(
            providers_support.contains(pattern),
            "handlers/admin/provider/ops/providers/support.rs should own {pattern}"
        );
    }

    for path in [
        "apps/aether-gateway/src/maintenance/runtime.rs",
        "apps/aether-gateway/src/maintenance/runtime/provider_checkin.rs",
    ] {
        let contents = read_workspace_file(path);
        assert!(
            (contents.contains("admin_api::admin_provider_ops_local_action_response")
                || (contents.contains("use crate::admin_api::{")
                    && contents.contains("admin_provider_ops_local_action_response"))),
            "{path} should call provider ops action helper through crate::admin_api facade"
        );
        assert!(
            !contents.contains(
                "provider::ops::providers::actions::admin_provider_ops_local_action_response"
            ) && !contents
                .contains("provider::ops::providers::admin_provider_ops_local_action_response"),
            "{path} should not depend on provider ops internal module paths"
        );
    }
}

#[test]
fn admin_provider_ops_actions_mod_stays_thin() {
    let actions_mod = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/ops/providers/actions/mod.rs",
    );
    for pattern in [
        "mod checkin;",
        "mod query_balance;",
        "mod responses;",
        "mod support;",
        "pub(super) fn admin_provider_ops_is_valid_action_type(",
        "pub(crate) async fn admin_provider_ops_local_action_response(",
    ] {
        assert!(
            actions_mod.contains(pattern),
            "handlers/admin/provider/ops/providers/actions/mod.rs should keep thin entry seam {pattern}"
        );
    }
    for forbidden in [
        "fn admin_provider_ops_action_response(",
        "fn admin_provider_ops_checkin_payload(",
        "fn admin_provider_ops_new_api_balance_payload(",
        "fn admin_provider_ops_yescode_balance_payload(",
        "fn admin_provider_ops_run_checkin_action(",
        "fn admin_provider_ops_run_query_balance_action(",
    ] {
        assert!(
            !actions_mod.contains(forbidden),
            "handlers/admin/provider/ops/providers/actions/mod.rs should not keep helper owner {forbidden}"
        );
    }
    assert!(
        !workspace_file_exists("apps/aether-gateway/src/handlers/admin/provider/ops/providers/actions.rs"),
        "handlers/admin/provider/ops/providers/actions.rs should be removed once actions logic is directoryized"
    );

    let actions_responses = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/ops/providers/actions/responses.rs",
    );
    for pattern in [
        "pub(super) fn admin_provider_ops_action_response(",
        "pub(super) fn admin_provider_ops_action_error(",
        "pub(super) fn admin_provider_ops_action_not_configured(",
        "pub(super) fn admin_provider_ops_action_not_supported(",
    ] {
        assert!(
            actions_responses.contains(pattern),
            "handlers/admin/provider/ops/providers/actions/responses.rs should own {pattern}"
        );
    }

    let actions_support = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/ops/providers/actions/support.rs",
    );
    for pattern in [
        "pub(super) fn admin_provider_ops_checkin_data(",
        "pub(super) fn admin_provider_ops_json_object_map(",
        "pub(super) fn admin_provider_ops_request_url(",
        "pub(super) fn admin_provider_ops_request_method(",
        "pub(super) fn admin_provider_ops_parse_rfc3339_unix_secs(",
    ] {
        assert!(
            actions_support.contains(pattern),
            "handlers/admin/provider/ops/providers/actions/support.rs should own {pattern}"
        );
    }

    let actions_checkin_mod = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/ops/providers/actions/checkin/mod.rs",
    );
    for pattern in [
        "mod probe;",
        "mod run;",
        "mod shared;",
        "pub(super) use probe::admin_provider_ops_probe_new_api_checkin;",
        "pub(super) use run::admin_provider_ops_run_checkin_action;",
    ] {
        assert!(
            actions_checkin_mod.contains(pattern),
            "handlers/admin/provider/ops/providers/actions/checkin/mod.rs should keep thin checkin entry seam {pattern}"
        );
    }
    let actions_checkin_shared = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/ops/providers/actions/checkin/shared.rs",
    );
    for pattern in [
        "pub(super) fn admin_provider_ops_checkin_already_done(",
        "pub(super) fn admin_provider_ops_checkin_auth_failure(",
        "pub(super) fn admin_provider_ops_checkin_payload(",
    ] {
        assert!(
            actions_checkin_shared.contains(pattern),
            "handlers/admin/provider/ops/providers/actions/checkin/shared.rs should own {pattern}"
        );
    }
    let actions_checkin_probe = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/ops/providers/actions/checkin/probe.rs",
    );
    assert!(
        actions_checkin_probe.contains("async fn admin_provider_ops_probe_new_api_checkin("),
        "handlers/admin/provider/ops/providers/actions/checkin/probe.rs should own new-api probe flow"
    );
    let actions_checkin_run = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/ops/providers/actions/checkin/run.rs",
    );
    assert!(
        actions_checkin_run.contains("async fn admin_provider_ops_run_checkin_action("),
        "handlers/admin/provider/ops/providers/actions/checkin/run.rs should own checkin action execution"
    );
    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/handlers/admin/provider/ops/providers/actions/checkin.rs"
        ),
        "handlers/admin/provider/ops/providers/actions/checkin.rs should be removed once checkin is directoryized"
    );

    let actions_query_balance_mod = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/ops/providers/actions/query_balance/mod.rs",
    );
    for pattern in [
        "mod sub2api;",
        "mod yescode;",
        "pub(super) async fn admin_provider_ops_run_query_balance_action(",
        "parse_query_balance_payload(",
        "yescode::admin_provider_ops_yescode_balance_payload(",
        "sub2api::admin_provider_ops_sub2api_balance_payload(",
    ] {
        assert!(
            actions_query_balance_mod.contains(pattern),
            "handlers/admin/provider/ops/providers/actions/query_balance/mod.rs should keep thin query_balance entry seam {pattern}"
        );
    }
    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/handlers/admin/provider/ops/providers/actions/query_balance/parsers.rs"
        ),
        "handlers/admin/provider/ops/providers/actions/query_balance/parsers.rs should be removed after moving balance parsing into aether-admin"
    );
    let pure_actions = read_workspace_file("crates/aether-admin/src/provider/ops/actions.rs");
    for pattern in [
        "pub fn parse_query_balance_payload(",
        "pub fn parse_sub2api_balance_payload(",
        "pub fn parse_yescode_combined_balance_payload(",
        "pub fn attach_balance_checkin_outcome(",
    ] {
        assert!(
            pure_actions.contains(pattern),
            "crates/aether-admin/src/provider/ops/actions.rs should own {pattern}"
        );
    }
    let actions_query_balance_yescode = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/ops/providers/actions/query_balance/yescode.rs",
    );
    assert!(
        actions_query_balance_yescode
            .contains("pub(super) async fn admin_provider_ops_yescode_balance_payload("),
        "handlers/admin/provider/ops/providers/actions/query_balance/yescode.rs should own yescode balance flow"
    );
    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/handlers/admin/provider/ops/providers/actions/query_balance.rs"
        ),
        "handlers/admin/provider/ops/providers/actions/query_balance.rs should be removed once query_balance is directoryized"
    );
}

#[test]
fn admin_provider_ops_verify_runtime_and_pure_owners_stay_explicit() {
    let verify_mod = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/ops/providers/verify/mod.rs",
    );
    for pattern in [
        "mod proxy;",
        "mod request;",
        "mod sub2api;",
        "pub(super) async fn admin_provider_ops_local_verify_response(",
    ] {
        assert!(
            verify_mod.contains(pattern),
            "handlers/admin/provider/ops/providers/verify/mod.rs should keep runtime verify entry seam {pattern}"
        );
    }

    let verify_proxy = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/ops/providers/verify/proxy.rs",
    );
    for pattern in [
        "struct AdminProviderOpsAnyrouterChallenge",
        "fn admin_provider_ops_anyrouter_acw_cookie(",
        "fn admin_provider_ops_resolve_proxy_snapshot(",
    ] {
        assert!(
            verify_proxy.contains(pattern),
            "handlers/admin/provider/ops/providers/verify/proxy.rs should own {pattern}"
        );
    }

    let verify_request = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/ops/providers/verify/request.rs",
    );
    for pattern in [
        "fn admin_provider_ops_execute_get_json(",
        "fn admin_provider_ops_execute_proxy_json_request(",
        "fn admin_provider_ops_verify_execution_error_message(",
    ] {
        assert!(
            verify_request.contains(pattern),
            "handlers/admin/provider/ops/providers/verify/request.rs should own {pattern}"
        );
    }

    let verify_sub2api = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/ops/providers/verify/sub2api.rs",
    );
    for pattern in [
        "fn admin_provider_ops_local_sub2api_verify_response(",
        "fn admin_provider_ops_sub2api_exchange_token(",
    ] {
        assert!(
            verify_sub2api.contains(pattern),
            "handlers/admin/provider/ops/providers/verify/sub2api.rs should own {pattern}"
        );
    }

    for path in [
        "apps/aether-gateway/src/handlers/admin/provider/ops/providers/verify/helpers.rs",
        "apps/aether-gateway/src/handlers/admin/provider/ops/providers/verify/headers.rs",
        "apps/aether-gateway/src/handlers/admin/provider/ops/providers/verify/payload.rs",
    ] {
        assert!(
            !workspace_file_exists(path),
            "{path} should be removed after splitting verify runtime owners"
        );
    }

    let pure_verify = read_workspace_file("crates/aether-admin/src/provider/ops/verify.rs");
    for pattern in ["pub fn build_headers(", "pub fn parse_verify_payload("] {
        assert!(
            pure_verify.contains(pattern),
            "crates/aether-admin/src/provider/ops/verify.rs should own {pattern}"
        );
    }
}

#[test]
fn admin_provider_oauth_dispatch_uses_helper_owner() {
    let dispatch_mod = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/mod.rs",
    );
    assert!(
        dispatch_mod.contains("mod helpers;"),
        "handlers/admin/provider/oauth/dispatch/mod.rs should register dispatch::helpers"
    );
    assert!(
        !dispatch_mod.contains("fn attach_admin_provider_oauth_audit_response("),
        "handlers/admin/provider/oauth/dispatch/mod.rs should not own dispatch audit helper implementation"
    );
    assert!(
        dispatch_mod.contains("helpers::attach_admin_provider_oauth_audit_response("),
        "handlers/admin/provider/oauth/dispatch/mod.rs should delegate audit attachment to dispatch::helpers"
    );

    let dispatch_helpers = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/helpers.rs",
    );
    assert!(
        dispatch_helpers.contains("pub(super) fn attach_admin_provider_oauth_audit_response("),
        "handlers/admin/provider/oauth/dispatch/helpers.rs should own dispatch audit helper"
    );
}

#[test]
fn admin_provider_oauth_state_mod_stays_thin() {
    let state_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/oauth/state/mod.rs");
    for pattern in [
        "mod auth_config;",
        "mod storage;",
        "pub(crate) use aether_admin::provider::state::{",
        "enrich_admin_provider_oauth_auth_config",
        "build_provider_oauth_start_response",
    ] {
        assert!(
            state_mod.contains(pattern),
            "handlers/admin/provider/oauth/state/mod.rs should keep explicit oauth state boundary {pattern}"
        );
    }
    for forbidden in [
        "const KIRO_DEVICE_DEFAULT_START_URL:",
        "const KIRO_DEVICE_DEFAULT_REGION:",
        "const KIRO_IDC_AMZ_USER_AGENT:",
        "pub(crate) async fn save_provider_oauth_device_session(",
        "pub(crate) async fn read_provider_oauth_device_session(",
        "pub(crate) async fn register_admin_kiro_device_oidc_client(",
        "pub(crate) async fn start_admin_kiro_device_authorization(",
        "pub(crate) async fn poll_admin_kiro_device_token(",
        "pub(crate) fn enrich_admin_provider_oauth_auth_config(",
        "pub(crate) async fn save_provider_oauth_batch_task_payload(",
        "pub(crate) async fn read_provider_oauth_batch_task_payload(",
    ] {
        assert!(
            !state_mod.contains(forbidden),
            "handlers/admin/provider/oauth/state/mod.rs should not retain concrete oauth-state owner {forbidden}"
        );
    }

    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/handlers/admin/provider/oauth/state/device.rs"
        ),
        "handlers/admin/provider/oauth/state/device.rs should be removed once oauth stateful owner moves into request/provider/oauth.rs"
    );

    let state_auth_config = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/state/auth_config.rs",
    );
    assert!(
        state_auth_config.contains("pub(crate) use aether_admin::provider::state::{"),
        "handlers/admin/provider/oauth/state/auth_config.rs should thin-bridge pure auth config helpers from aether_admin::provider::state"
    );
    for pattern in [
        "json_non_empty_string",
        "json_u64_value",
        "decode_jwt_claims",
        "enrich_admin_provider_oauth_auth_config",
    ] {
        assert!(
            state_auth_config.contains(pattern),
            "handlers/admin/provider/oauth/state/auth_config.rs should continue exporting {pattern}"
        );
    }

    let state_storage = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/state/storage.rs",
    );
    assert!(
        state_storage.contains("pub(crate) fn build_provider_oauth_start_response("),
        "handlers/admin/provider/oauth/state/storage.rs should keep the pure start-response builder"
    );
    for forbidden in [
        "pub(crate) async fn save_provider_oauth_state(",
        "pub(crate) async fn consume_provider_oauth_state(",
        "pub(crate) async fn save_provider_oauth_batch_task_payload(",
        "pub(crate) async fn read_provider_oauth_batch_task_payload(",
    ] {
        assert!(
            !state_storage.contains(forbidden),
            "handlers/admin/provider/oauth/state/storage.rs should not keep migrated stateful owner {forbidden}"
        );
    }

    assert!(
        !workspace_file_exists("apps/aether-gateway/src/handlers/admin/provider/oauth/state.rs"),
        "handlers/admin/provider/oauth/state.rs should be removed once oauth::state is directoryized"
    );
}

#[test]
fn admin_provider_oauth_state_template_exchange_split() {
    let state_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/oauth/state/mod.rs");
    for pattern in [
        "mod template;",
        "mod exchange;",
        "pub(crate) use self::template::{",
        "pub(crate) use self::exchange::{",
    ] {
        assert!(
            state_mod.contains(pattern),
            "handlers/admin/provider/oauth/state/mod.rs should expose template/exchange boundary {pattern}"
        );
    }

    for pattern in [
        "admin_provider_oauth_template",
        "build_admin_provider_oauth_supported_types_payload",
        "build_admin_provider_oauth_backend_unavailable_response",
        "is_fixed_provider_type_for_provider_oauth",
    ] {
        assert!(
            state_mod.contains(pattern),
            "handlers/admin/provider/oauth/state/mod.rs should re-export template helper {pattern}"
        );
    }

    for pattern in [
        "exchange_admin_provider_oauth_code",
        "exchange_admin_provider_oauth_refresh_token",
    ] {
        assert!(
            state_mod.contains(pattern),
            "handlers/admin/provider/oauth/state/mod.rs should re-export exchange helper {pattern}"
        );
    }

    for pattern in [
        "admin_provider_oauth_template",
        "build_admin_provider_oauth_supported_types_payload",
        "build_admin_provider_oauth_backend_unavailable_response",
        "is_fixed_provider_type_for_provider_oauth",
    ] {
        assert!(
            state_mod.contains(pattern),
            "handlers/admin/provider/oauth/state/mod.rs should re-export template helper {pattern}"
        );
    }

    for pattern in [
        "exchange_admin_provider_oauth_code",
        "exchange_admin_provider_oauth_refresh_token",
    ] {
        assert!(
            state_mod.contains(pattern),
            "handlers/admin/provider/oauth/state/mod.rs should re-export exchange helper {pattern}"
        );
    }

    for pattern in [
        "admin_provider_oauth_template",
        "build_admin_provider_oauth_supported_types_payload",
        "build_admin_provider_oauth_backend_unavailable_response",
        "is_fixed_provider_type_for_provider_oauth",
    ] {
        assert!(
            state_mod.contains(pattern),
            "handlers/admin/provider/oauth/state/mod.rs should re-export template helper {pattern}"
        );
    }

    for pattern in [
        "exchange_admin_provider_oauth_code",
        "exchange_admin_provider_oauth_refresh_token",
    ] {
        assert!(
            state_mod.contains(pattern),
            "handlers/admin/provider/oauth/state/mod.rs should re-export exchange helper {pattern}"
        );
    }

    let template = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/state/template.rs",
    );
    for pattern in [
        "pub(crate) fn is_fixed_provider_type_for_provider_oauth(",
        "pub(crate) fn admin_provider_oauth_template(",
        "pub(crate) fn build_admin_provider_oauth_supported_types_payload(",
        "pub(crate) fn build_admin_provider_oauth_backend_unavailable_response(",
    ] {
        assert!(
            template.contains(pattern),
            "handlers/admin/provider/oauth/state/template.rs should own template helper {pattern}"
        );
    }

    let exchange = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/state/exchange.rs",
    );
    for pattern in [
        "pub(crate) async fn exchange_admin_provider_oauth_code(",
        "pub(crate) async fn exchange_admin_provider_oauth_refresh_token(",
    ] {
        assert!(
            exchange.contains(pattern),
            "handlers/admin/provider/oauth/state/exchange.rs should own exchange helper {pattern}"
        );
    }

    assert!(
        !state_mod.contains("pub(crate) fn admin_provider_oauth_template("),
        "state/mod.rs should not define template helpers once split"
    );
    assert!(
        !state_mod.contains("pub(crate) async fn exchange_admin_provider_oauth_code("),
        "state/mod.rs should not define exchange helpers once split"
    );
}

#[test]
fn admin_provider_oauth_quota_mod_stays_thin() {
    let quota_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/oauth/quota/mod.rs");
    for pattern in [
        "pub(crate) mod antigravity;",
        "pub(crate) mod codex;",
        "pub(crate) mod kiro;",
        "pub(crate) mod shared;",
    ] {
        assert!(
            quota_mod.contains(pattern),
            "handlers/admin/provider/oauth/quota/mod.rs should expose explicit quota owner {pattern}"
        );
    }
    for forbidden in [
        "pub(crate) use self::antigravity::refresh_antigravity_provider_quota_locally;",
        "pub(crate) use self::codex::refresh_codex_provider_quota_locally;",
        "pub(crate) use self::kiro::refresh_kiro_provider_quota_locally;",
        "pub(crate) use self::shared::{normalize_string_id_list, persist_provider_quota_refresh_state};",
        "use self::shared::{",
    ] {
        assert!(
            !quota_mod.contains(forbidden),
            "handlers/admin/provider/oauth/quota/mod.rs should not remain a quota helper export hub for {forbidden}"
        );
    }

    let endpoint_keys_quota = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/endpoint_keys/quota.rs",
    );
    for pattern in [
        "use super::super::oauth::quota::antigravity::refresh_antigravity_provider_quota_locally;",
        "use super::super::oauth::quota::codex::refresh_codex_provider_quota_locally;",
        "use super::super::oauth::quota::kiro::refresh_kiro_provider_quota_locally;",
        "use super::super::oauth::quota::shared::normalize_string_id_list;",
    ] {
        assert!(
            endpoint_keys_quota.contains(pattern),
            "handlers/admin/provider/endpoint_keys/quota.rs should import quota helper via explicit owner {pattern}"
        );
    }

    let oauth_runtime =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/oauth/runtime.rs");
    for pattern in [
        "use super::quota::antigravity::refresh_antigravity_provider_quota_locally;",
        "use super::quota::codex::refresh_codex_provider_quota_locally;",
        "use super::quota::kiro::refresh_kiro_provider_quota_locally;",
    ] {
        assert!(
            oauth_runtime.contains(pattern),
            "handlers/admin/provider/oauth/runtime.rs should import quota helper via explicit owner {pattern}"
        );
    }

    let quota_codex_mod = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/quota/codex/mod.rs",
    );
    for pattern in [
        "mod invalid;",
        "mod parse;",
        "mod plan;",
        "use self::invalid::{",
        "use self::parse::{",
        "use self::plan::{",
        "use super::shared::{",
        "pub(crate) async fn refresh_codex_provider_quota_locally(",
    ] {
        assert!(
            quota_codex_mod.contains(pattern),
            "handlers/admin/provider/oauth/quota/codex/mod.rs should keep explicit codex quota owner boundary {pattern}"
        );
    }
    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/handlers/admin/provider/oauth/quota/codex.rs"
        ),
        "handlers/admin/provider/oauth/quota/codex.rs should be removed once codex quota is directoryized"
    );
    let quota_codex_parse = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/quota/codex/parse.rs",
    );
    assert!(
        quota_codex_parse.contains("use super::super::shared::{")
            || quota_codex_parse.contains("use aether_admin::provider::quota"),
        "handlers/admin/provider/oauth/quota/codex/parse.rs should either own local shared parsing helpers or delegate to aether-admin"
    );
    let quota_codex_invalid = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/quota/codex/invalid.rs",
    );
    assert!(
        quota_codex_invalid.contains(
            "use crate::handlers::admin::provider::shared::payloads::{"
        ) || quota_codex_invalid.contains("use aether_admin::provider::quota"),
        "handlers/admin/provider/oauth/quota/codex/invalid.rs should either own codex invalid-state helpers locally or delegate to aether-admin"
    );
    let quota_codex_plan = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/quota/codex/plan.rs",
    );
    for pattern in [
        "use super::parse::normalize_codex_plan_type;",
        "use crate::handlers::admin::provider::shared::payloads::CODEX_WHAM_USAGE_URL;",
        "pub(super) fn build_codex_refresh_headers(",
        "pub(super) async fn execute_codex_quota_plan(",
    ] {
        assert!(
            quota_codex_plan.contains(pattern),
            "handlers/admin/provider/oauth/quota/codex/plan.rs should own codex quota execution helper {pattern}"
        );
    }
    let quota_kiro_mod = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/quota/kiro/mod.rs",
    );
    for pattern in [
        "mod parse;",
        "mod plan;",
        "use self::parse::parse_kiro_usage_response;",
        "use self::plan::execute_kiro_quota_plan;",
        "use super::shared::{",
        "pub(crate) async fn refresh_kiro_provider_quota_locally(",
    ] {
        assert!(
            quota_kiro_mod.contains(pattern),
            "handlers/admin/provider/oauth/quota/kiro/mod.rs should keep explicit kiro quota owner boundary {pattern}"
        );
    }
    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/handlers/admin/provider/oauth/quota/kiro.rs"
        ),
        "handlers/admin/provider/oauth/quota/kiro.rs should be removed once kiro quota is directoryized"
    );
    let quota_kiro_parse = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/quota/kiro/parse.rs",
    );
    assert!(
        quota_kiro_parse.contains("use super::super::shared::coerce_json_f64;")
            || quota_kiro_parse.contains("use aether_admin::provider::quota"),
        "handlers/admin/provider/oauth/quota/kiro/parse.rs should either own local shared parsing helpers or delegate to aether-admin"
    );
    let quota_kiro_plan = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/quota/kiro/plan.rs",
    );
    for pattern in [
        "use super::super::shared::{execute_provider_quota_plan, ProviderQuotaExecutionOutcome};",
        "use crate::handlers::admin::provider::shared::payloads::{",
        "pub(super) async fn execute_kiro_quota_plan(",
    ] {
        assert!(
            quota_kiro_plan.contains(pattern),
            "handlers/admin/provider/oauth/quota/kiro/plan.rs should own kiro quota execution helper {pattern}"
        );
    }
    let quota_antigravity = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/quota/antigravity.rs",
    );
    assert!(
        quota_antigravity.contains("use super::shared::{"),
        "handlers/admin/provider/oauth/quota/antigravity.rs should import common quota helpers from shared.rs"
    );
}

#[test]
fn admin_provider_oauth_refresh_helpers_use_specific_local_owners() {
    let oauth_errors =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/oauth/errors.rs");
    for pattern in [
        "pub(crate) fn build_internal_control_error_response(",
        "pub(crate) fn normalize_provider_oauth_refresh_error_message(",
        "pub(crate) fn merge_provider_oauth_refresh_failure_reason(",
    ] {
        assert!(
            oauth_errors.contains(pattern),
            "handlers/admin/provider/oauth/errors.rs should own {pattern}"
        );
    }

    let oauth_duplicates =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/oauth/duplicates.rs");
    for pattern in [
        "fn normalize_codex_plan_group_for_provider_oauth(",
        "fn normalize_provider_oauth_identity_value(",
        "fn is_codex_provider_oauth_provider_type(",
        "fn match_codex_provider_oauth_identity(",
        "fn is_codex_cross_plan_group_non_duplicate(",
        "pub(crate) async fn find_duplicate_provider_oauth_key(",
    ] {
        assert!(
            oauth_duplicates.contains(pattern),
            "handlers/admin/provider/oauth/duplicates.rs should own {pattern}"
        );
    }

    let oauth_provisioning = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/provisioning.rs",
    );
    for pattern in [
        "pub(crate) fn provider_oauth_key_proxy_value(",
        "pub(crate) fn provider_oauth_active_api_formats(",
        "pub(crate) fn build_provider_oauth_auth_config_from_token_payload(",
        "pub(crate) async fn create_provider_oauth_catalog_key(",
        "pub(crate) async fn update_existing_provider_oauth_catalog_key(",
    ] {
        assert!(
            oauth_provisioning.contains(pattern),
            "handlers/admin/provider/oauth/provisioning.rs should own {pattern}"
        );
    }

    let oauth_runtime =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/oauth/runtime.rs");
    for pattern in [
        "pub(crate) fn provider_oauth_runtime_endpoint_for_provider(",
        "pub(crate) async fn refresh_provider_oauth_account_state_after_update(",
    ] {
        assert!(
            oauth_runtime.contains(pattern),
            "handlers/admin/provider/oauth/runtime.rs should own {pattern}"
        );
    }

    assert!(
        !workspace_file_exists("apps/aether-gateway/src/handlers/admin/provider/oauth/refresh.rs"),
        "handlers/admin/provider/oauth/refresh.rs should be removed once oauth helper owners are split"
    );
}

#[test]
fn admin_provider_oauth_dispatch_batch_mod_stays_thin() {
    let batch_mod = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/batch/mod.rs",
    );
    for pattern in [
        "mod execution;",
        "mod kiro_import;",
        "mod orchestration;",
        "mod parse;",
        "mod task;",
        "pub(super) use orchestration::handle_admin_provider_oauth_batch_import;",
        "pub(super) use task::handle_admin_provider_oauth_start_batch_import_task;",
    ] {
        assert!(
            batch_mod.contains(pattern),
            "handlers/admin/provider/oauth/dispatch/batch/mod.rs should keep explicit boundary {pattern}"
        );
    }
    for forbidden in [
        "struct AdminProviderOAuthBatchImportEntry",
        "fn parse_admin_provider_oauth_batch_import_request",
        "fn build_kiro_batch_import_key_name",
        "fn parse_admin_provider_oauth_kiro_batch_import_entries",
        "fn normalize_admin_provider_oauth_kiro_import_item",
        "async fn execute_admin_provider_oauth_batch_import(",
    ] {
        assert!(
            !batch_mod.contains(forbidden),
            "handlers/admin/provider/oauth/dispatch/batch/mod.rs should not keep helper implementations for {forbidden}"
        );
    }
    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/batch.rs"
        ),
        "dispatch/batch.rs should be removed once batch dispatch is directoryized"
    );

    let batch_parse = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/batch/parse.rs",
    );
    for pattern in [
        "pub(super) struct AdminProviderOAuthBatchImportRequest",
        "pub(super) struct AdminProviderOAuthBatchImportEntry",
        "pub(super) struct AdminProviderOAuthBatchImportOutcome",
        "pub(super) fn parse_admin_provider_oauth_batch_import_request(",
        "pub(super) fn parse_admin_provider_oauth_batch_import_entries(",
        "pub(super) fn apply_admin_provider_oauth_batch_import_hints(",
        "pub(super) async fn extract_admin_provider_oauth_batch_error_detail(",
        "pub(super) fn build_admin_provider_oauth_batch_import_response(",
        "pub(super) fn build_admin_provider_oauth_batch_task_state(",
    ] {
        assert!(
            batch_parse.contains(pattern),
            "handlers/admin/provider/oauth/dispatch/batch/parse.rs should own {pattern}"
        );
    }

    let batch_kiro_import = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/batch/kiro_import.rs",
    );
    assert!(
        batch_kiro_import.contains("pub(super) async fn execute_admin_provider_oauth_kiro_batch_import("),
        "handlers/admin/provider/oauth/dispatch/batch/kiro_import.rs should own the kiro batch execution owner"
    );
    assert!(
        batch_kiro_import.contains("build_kiro_batch_import_key_name(")
            || batch_kiro_import.contains("aether_admin::provider::oauth::build_kiro_batch_import_key_name"),
        "handlers/admin/provider/oauth/dispatch/batch/kiro_import.rs should either own or delegate the kiro key-name builder"
    );
    assert!(
        batch_kiro_import.contains("parse_admin_provider_oauth_kiro_batch_import_entries(")
            || batch_kiro_import.contains("aether_admin::provider::oauth::parse_admin_provider_oauth_kiro_batch_import_entries"),
        "handlers/admin/provider/oauth/dispatch/batch/kiro_import.rs should either own or delegate the kiro import parser"
    );

    let batch_execution = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/batch/execution.rs",
    );
    for pattern in [
        "pub(super) fn estimate_admin_provider_oauth_batch_import_total(",
        "pub(super) async fn execute_admin_provider_oauth_batch_import_for_provider_type(",
        "pub(super) async fn execute_admin_provider_oauth_batch_import(",
    ] {
        assert!(
            batch_execution.contains(pattern),
            "handlers/admin/provider/oauth/dispatch/batch/execution.rs should own {pattern}"
        );
    }

    let batch_orchestration = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/batch/orchestration.rs",
    );
    assert!(
        batch_orchestration.contains("pub(in super::super) async fn handle_admin_provider_oauth_batch_import("),
        "handlers/admin/provider/oauth/dispatch/batch/orchestration.rs should own the direct batch import route"
    );

    let batch_task = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/batch/task.rs",
    );
    assert!(
        batch_task.contains(
            "pub(in super::super) async fn handle_admin_provider_oauth_start_batch_import_task("
        ),
        "handlers/admin/provider/oauth/dispatch/batch/task.rs should own the async batch task route"
    );
}

#[test]
fn admin_provider_oauth_dispatch_refresh_mod_stays_thin() {
    let refresh_mod = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/refresh.rs",
    );
    for pattern in [
        "mod execution;",
        "mod helpers;",
        "mod request;",
        "mod response;",
        "request::parse_admin_provider_oauth_refresh_request(",
        "execution::execute_admin_provider_oauth_refresh(",
        "response::admin_provider_oauth_refresh_success_response(",
    ] {
        assert!(
            refresh_mod.contains(pattern),
            "handlers/admin/provider/oauth/dispatch/refresh.rs should keep explicit refresh boundary {pattern}"
        );
    }
    for forbidden in [
        "fn decrypt_auth_config(",
        "fn parse_auth_config_object(",
        "fn refreshed_auth_config_object(",
        "fn auth_config_has_refresh_token(",
        "fn key_is_account_blocked(",
        "fn unix_now_secs(",
    ] {
        assert!(
            !refresh_mod.contains(forbidden),
            "handlers/admin/provider/oauth/dispatch/refresh.rs should not keep concrete helper implementations for {forbidden}"
        );
    }

    let refresh_request = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/refresh/request.rs",
    );
    assert!(
        refresh_request.contains("pub(super) async fn parse_admin_provider_oauth_refresh_request("),
        "handlers/admin/provider/oauth/dispatch/refresh/request.rs should own request parsing"
    );

    let refresh_execution = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/refresh/execution.rs",
    );
    assert!(
        refresh_execution.contains("pub(super) async fn execute_admin_provider_oauth_refresh("),
        "handlers/admin/provider/oauth/dispatch/refresh/execution.rs should own refresh execution"
    );

    let refresh_response = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/refresh/response.rs",
    );
    for pattern in [
        "pub(super) fn control_error_response(",
        "pub(super) fn oauth_refresh_failed_bad_request_response(",
        "pub(super) fn oauth_refresh_failed_service_unavailable_response(",
        "pub(super) fn admin_provider_oauth_refresh_success_response(",
    ] {
        assert!(
            refresh_response.contains(pattern),
            "handlers/admin/provider/oauth/dispatch/refresh/response.rs should own {pattern}"
        );
    }

    let refresh_helpers = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/refresh/helpers.rs",
    );
    for pattern in [
        "pub(super) enum RefreshDispatch<T> {",
        "pub(super) struct RefreshRequestContext {",
        "pub(super) struct RefreshSuccessContext {",
        "pub(super) fn decrypt_auth_config(",
        "pub(super) fn parse_auth_config_object(",
        "pub(super) fn refreshed_auth_config_object(",
        "pub(super) fn auth_config_has_refresh_token(",
        "pub(super) fn key_is_account_blocked(",
        "pub(super) fn unix_now_secs(",
    ] {
        assert!(
            refresh_helpers.contains(pattern),
            "handlers/admin/provider/oauth/dispatch/refresh/helpers.rs should own {pattern}"
        );
    }
}

#[test]
fn admin_provider_oauth_device_mod_stays_thin() {
    let device_mod = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/device/mod.rs",
    );
    for pattern in [
        "mod authorize;",
        "mod poll;",
        "mod session;",
        "pub(super) async fn handle_admin_provider_oauth_device_authorize(",
        "authorize::handle_admin_provider_oauth_device_authorize(",
        "pub(super) async fn handle_admin_provider_oauth_device_poll(",
        "poll::handle_admin_provider_oauth_device_poll(",
    ] {
        assert!(
            device_mod.contains(pattern),
            "handlers/admin/provider/oauth/dispatch/device/mod.rs should keep explicit boundary {pattern}"
        );
    }
    for forbidden in [
        "struct AdminProviderOAuthDeviceAuthorizePayload",
        "struct AdminProviderOAuthDevicePollPayload",
        "fn attach_admin_provider_oauth_device_poll_terminal_response(",
    ] {
        assert!(
            !device_mod.contains(forbidden),
            "handlers/admin/provider/oauth/dispatch/device/mod.rs should not keep concrete device helper {forbidden}"
        );
    }

    let device_authorize = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/device/authorize.rs",
    );
    for pattern in [
        "use super::session::AdminProviderOAuthDeviceAuthorizePayload;",
        "pub(super) async fn handle_admin_provider_oauth_device_authorize(",
    ] {
        assert!(
            device_authorize.contains(pattern),
            "handlers/admin/provider/oauth/dispatch/device/authorize.rs should own authorize flow {pattern}"
        );
    }

    let device_poll = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/device/poll.rs",
    );
    for pattern in [
        "use super::session::{",
        "pub(super) async fn handle_admin_provider_oauth_device_poll(",
    ] {
        assert!(
            device_poll.contains(pattern),
            "handlers/admin/provider/oauth/dispatch/device/poll.rs should own poll flow {pattern}"
        );
    }

    let device_session = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/device/session.rs",
    );
    for pattern in [
        "pub(super) struct AdminProviderOAuthDeviceAuthorizePayload",
        "pub(super) struct AdminProviderOAuthDevicePollPayload",
        "pub(super) fn attach_admin_provider_oauth_device_poll_terminal_response(",
    ] {
        assert!(
            device_session.contains(pattern),
            "handlers/admin/provider/oauth/dispatch/device/session.rs should own {pattern}"
        );
    }

    assert!(
        !workspace_file_exists("apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/device.rs"),
        "handlers/admin/provider/oauth/dispatch/device.rs should be removed once device dispatch is directoryized"
    );
}

#[test]
fn admin_provider_endpoint_keys_mod_stays_thin() {
    let endpoint_keys =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/endpoint_keys.rs");
    for pattern in [
        "mod mutations;",
        "mod quota;",
        "mod reads;",
        "reads::maybe_handle(",
        "mutations::maybe_handle(",
        "quota::maybe_handle(",
    ] {
        assert!(
            endpoint_keys.contains(pattern),
            "handlers/admin/provider/endpoint_keys.rs should keep explicit endpoint-key boundary {pattern}"
        );
    }

    for forbidden in [
        "keys_grouped_by_format",
        "reveal_key",
        "export_key",
        "update_key",
        "delete_key",
        "batch_delete_keys",
        "clear_oauth_invalid",
        "refresh_quota",
        "create_provider_key",
        "list_provider_keys",
        "build_admin_keys_grouped_by_format_payload",
        "build_admin_create_provider_key_record",
        "refresh_codex_provider_quota_locally",
    ] {
        assert!(
            !endpoint_keys.contains(forbidden),
            "handlers/admin/provider/endpoint_keys.rs should not remain a route/helper hub for {forbidden}"
        );
    }

    let endpoint_keys_reads = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/endpoint_keys/reads.rs",
    );
    for pattern in [
        "keys_grouped_by_format",
        "reveal_key",
        "export_key",
        "list_provider_keys",
    ] {
        assert!(
            endpoint_keys_reads.contains(pattern),
            "handlers/admin/provider/endpoint_keys/reads.rs should own read route {pattern}"
        );
    }

    let endpoint_keys_mutations = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/endpoint_keys/mutations/mod.rs",
    );
    for pattern in [
        "update::maybe_handle(",
        "delete::maybe_handle(",
        "batch::maybe_handle(",
        "oauth_invalid::maybe_handle(",
        "create::maybe_handle(",
    ] {
        assert!(
            endpoint_keys_mutations.contains(pattern),
            "handlers/admin/provider/endpoint_keys/mutations/mod.rs should dispatch mutation route {pattern}"
        );
    }
    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/handlers/admin/provider/endpoint_keys/mutations.rs"
        ),
        "handlers/admin/provider/endpoint_keys/mutations.rs should be removed once endpoint-key mutations are directoryized"
    );
    for path in [
        "apps/aether-gateway/src/handlers/admin/provider/endpoint_keys/mutations/create.rs",
        "apps/aether-gateway/src/handlers/admin/provider/endpoint_keys/mutations/update.rs",
        "apps/aether-gateway/src/handlers/admin/provider/endpoint_keys/mutations/delete.rs",
        "apps/aether-gateway/src/handlers/admin/provider/endpoint_keys/mutations/batch.rs",
        "apps/aether-gateway/src/handlers/admin/provider/endpoint_keys/mutations/oauth_invalid.rs",
    ] {
        assert!(
            workspace_file_exists(path),
            "{path} should exist once endpoint-key mutations are split by route owner"
        );
    }

    let endpoint_keys_create = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/endpoint_keys/mutations/create.rs",
    );
    assert!(
        endpoint_keys_create
            .contains("super::super::super::write::keys::build_admin_create_provider_key_record")
            || endpoint_keys_create.contains(".build_admin_create_provider_key_record("),
        "create.rs should depend on the explicit key-write owner or wrapped admin state capability"
    );

    let endpoint_keys_update = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/endpoint_keys/mutations/update.rs",
    );
    assert!(
        endpoint_keys_update
            .contains("super::super::super::write::keys::build_admin_update_provider_key_record")
            || endpoint_keys_update.contains(".build_admin_update_provider_key_record("),
        "update.rs should depend on the explicit key-write owner or wrapped admin state capability"
    );

    let endpoint_keys_quota = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/endpoint_keys/quota.rs",
    );
    assert!(
        endpoint_keys_quota.contains("refresh_quota"),
        "handlers/admin/provider/endpoint_keys/quota.rs should own quota refresh route"
    );
}

#[test]
fn admin_provider_models_own_provider_model_builders() {
    let provider_models_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/models/mod.rs");
    {
        let pattern = "mod payloads;";
        assert!(
            provider_models_mod.contains(pattern),
            "handlers/admin/provider/models/mod.rs should register local provider-model owner module {pattern}"
        );
    }

    for path in [
        "apps/aether-gateway/src/handlers/admin/provider/models/list.rs",
        "apps/aether-gateway/src/handlers/admin/provider/models/detail.rs",
        "apps/aether-gateway/src/handlers/admin/provider/models/create.rs",
        "apps/aether-gateway/src/handlers/admin/provider/models/update.rs",
        "apps/aether-gateway/src/handlers/admin/provider/models/batch.rs",
        "apps/aether-gateway/src/handlers/admin/provider/models/import.rs",
        "apps/aether-gateway/src/handlers/admin/provider/models/available_source.rs",
        "apps/aether-gateway/src/handlers/admin/provider/models/assign_global.rs",
    ] {
        let contents = read_workspace_file(path);
        for forbidden in [
            "super::super::super::model::",
            "crate::handlers::admin::model::",
        ] {
            assert!(
                !contents.contains(forbidden),
                "{path} should not borrow provider-model builders from admin/model via {forbidden}"
            );
        }
    }

    let model_mod = read_workspace_file("apps/aether-gateway/src/handlers/admin/model/mod.rs");
    for forbidden in [
        "admin_provider_model_name_exists",
        "build_admin_provider_model_payload",
        "build_admin_provider_model_response",
        "build_admin_provider_models_payload",
        "build_admin_provider_model_create_record",
        "build_admin_provider_model_update_record",
        "build_admin_provider_available_source_models_payload",
        "build_admin_batch_assign_global_models_payload",
        "build_admin_import_provider_models_payload",
    ] {
        assert!(
            !model_mod.contains(forbidden),
            "handlers/admin/model/mod.rs should not export provider-model owner {forbidden}"
        );
    }
}

#[test]
fn admin_provider_models_write_is_absorbed_by_wrapped_state() {
    let models_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/models/mod.rs");
    assert!(
        !models_mod.contains("mod write;"),
        "handlers/admin/provider/models/mod.rs should no longer retain the transitional write module"
    );

    for path in [
        "apps/aether-gateway/src/handlers/admin/provider/models/write.rs",
        "apps/aether-gateway/src/handlers/admin/provider/models/write/mod.rs",
        "apps/aether-gateway/src/handlers/admin/provider/models/write/shared.rs",
        "apps/aether-gateway/src/handlers/admin/provider/models/write/records.rs",
        "apps/aether-gateway/src/handlers/admin/provider/models/write/imports.rs",
        "apps/aether-gateway/src/handlers/admin/provider/models/write/batch_assign.rs",
        "apps/aether-gateway/src/handlers/admin/provider/models/write/available_source.rs",
    ] {
        assert!(
            !workspace_file_exists(path),
            "{path} should be removed once provider-model write owners are absorbed by wrapped state"
        );
    }

    let request_models =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/request/models.rs");
    for pattern in [
        "pub(crate) async fn build_admin_provider_model_create_record(",
        "pub(crate) async fn build_admin_provider_model_update_record(",
        "pub(crate) async fn build_admin_import_provider_models_payload(",
        "pub(crate) async fn build_admin_batch_assign_global_models_payload(",
        "pub(crate) async fn build_admin_provider_available_source_models_payload(",
        "pub(crate) async fn admin_provider_model_name_exists(",
        "pub(crate) async fn resolve_admin_global_model_by_id_or_err(",
    ] {
        assert!(
            request_models.contains(pattern),
            "handlers/admin/request/models.rs should own {pattern}"
        );
    }

    for (path, expected) in [
        (
            "apps/aether-gateway/src/handlers/admin/provider/models/create.rs",
            ".build_admin_provider_model_create_record(",
        ),
        (
            "apps/aether-gateway/src/handlers/admin/provider/models/update.rs",
            ".build_admin_provider_model_update_record(",
        ),
        (
            "apps/aether-gateway/src/handlers/admin/provider/models/import.rs",
            ".build_admin_import_provider_models_payload(",
        ),
        (
            "apps/aether-gateway/src/handlers/admin/provider/models/assign_global.rs",
            ".build_admin_batch_assign_global_models_payload(",
        ),
        (
            "apps/aether-gateway/src/handlers/admin/provider/models/available_source.rs",
            ".build_admin_provider_available_source_models_payload(",
        ),
        (
            "apps/aether-gateway/src/handlers/admin/provider/models/batch.rs",
            ".build_admin_provider_model_create_record(",
        ),
    ] {
        let contents = read_workspace_file(path);
        assert!(
            contents.contains(expected),
            "{path} should delegate provider-model write flows through wrapped state {expected}"
        );
        assert!(
            !contents.contains("super::write::"),
            "{path} should not retain the removed provider/models/write seam"
        );
    }
}
