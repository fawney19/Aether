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
        "pub(crate) use self::models::maybe_build_local_admin_provider_models_response;",
        "pub(crate) use self::oauth::maybe_build_local_admin_provider_oauth_response;",
        "pub(crate) use self::ops::maybe_build_local_admin_provider_ops_response;",
        "pub(crate) use self::query::maybe_build_local_admin_provider_query_response;",
        "pub(crate) use self::strategy::maybe_build_local_admin_provider_strategy_response;",
    ] {
        assert!(
            provider_mod.contains(pattern),
            "handlers/admin/provider/mod.rs should keep route entry seam {pattern}"
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
        "pub(crate) use self::quota as provider_oauth_quota;",
        "pub(crate) use self::refresh as provider_oauth_refresh;",
        "pub(crate) use self::state as provider_oauth_state;",
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
        "mod read_routes;",
        "mod write_routes;",
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
    ] {
        assert!(
            workspace_file_exists(path),
            "{path} should exist once endpoints_admin dispatches through specific route owners"
        );
    }

    for path in [
        "apps/aether-gateway/src/handlers/admin/provider/endpoints_admin/read_routes.rs",
        "apps/aether-gateway/src/handlers/admin/provider/endpoints_admin/write_routes.rs",
    ] {
        assert!(
            !workspace_file_exists(path),
            "{path} should be deleted once endpoints_admin stops routing through read/write buses"
        );
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
        strategy_routes.contains("use super::responses::{"),
        "handlers/admin/provider/strategy/routes.rs should import route response helpers from strategy::responses"
    );
    assert!(
        !strategy_routes.contains("use super::shared::{"),
        "handlers/admin/provider/strategy/routes.rs should not depend on strategy::shared"
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
fn admin_provider_crud_uses_specific_local_response_owner() {
    let crud_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/crud/mod.rs");
    for pattern in ["mod responses;", "mod routes;"] {
        assert!(
            crud_mod.contains(pattern),
            "handlers/admin/provider/crud/mod.rs should register local owner {pattern}"
        );
    }
    for forbidden in ["mod shared;", "use shared::*;"] {
        assert!(
            !crud_mod.contains(forbidden),
            "handlers/admin/provider/crud/mod.rs should not retain generic shared glue {forbidden}"
        );
    }

    let crud_routes =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/crud/routes.rs");
    assert!(
        crud_routes.contains("use super::responses::build_admin_providers_data_unavailable_response;"),
        "handlers/admin/provider/crud/routes.rs should import data-unavailable response from crud::responses"
    );
    assert!(
        !crud_routes.contains("use super::shared::"),
        "handlers/admin/provider/crud/routes.rs should not depend on crud::shared"
    );

    assert!(
        workspace_file_exists("apps/aether-gateway/src/handlers/admin/provider/crud/responses.rs"),
        "handlers/admin/provider/crud/responses.rs should own provider CRUD response helpers"
    );
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

    let pool_runtime =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/pool/runtime.rs");
    for pattern in [
        "pub(crate) async fn read_admin_provider_pool_cooldown_counts(",
        "pub(crate) async fn read_admin_provider_pool_runtime_state(",
        "pub(crate) async fn build_admin_provider_pool_status_payload(",
        "pub(crate) async fn clear_admin_provider_pool_cooldown(",
        "pub(crate) async fn reset_admin_provider_pool_cost(",
    ] {
        assert!(
            pool_runtime.contains(pattern),
            "handlers/admin/provider/pool/runtime.rs should own pool runtime helper {pattern}"
        );
    }

    let pool_admin_read_routes = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/pool_admin/read_routes.rs",
    );
    for pattern in [
        "crate::handlers::admin::provider::pool::config::admin_provider_pool_config",
        "crate::handlers::admin::provider::pool::runtime::{",
    ] {
        assert!(
            pool_admin_read_routes.contains(pattern),
            "handlers/admin/provider/pool_admin/read_routes.rs should import explicit pool owner {pattern}"
        );
    }

    let crud_routes =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/crud/routes.rs");
    assert!(
        crud_routes.contains("crate::handlers::admin::provider::pool::runtime::{"),
        "handlers/admin/provider/crud/routes.rs should import pool status and mutations from pool::runtime"
    );
}

#[test]
fn admin_provider_pool_admin_mod_stays_thin() {
    let pool_admin_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/pool_admin/mod.rs");
    for pattern in [
        "mod support;",
        "mod batch_routes;",
        "mod payloads;",
        "mod read_routes;",
        "mod selection;",
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
        "pub(super) const ADMIN_POOL_PROVIDER_CATALOG_READER_UNAVAILABLE_DETAIL:",
        "pub(super) const ADMIN_POOL_PROVIDER_CATALOG_WRITER_UNAVAILABLE_DETAIL:",
        "pub(super) const ADMIN_POOL_BANNED_KEY_CLEANUP_EMPTY_MESSAGE:",
        "pub(super) struct AdminPoolResolveSelectionRequest",
        "pub(super) fn parse_admin_pool_page(",
        "pub(super) fn parse_admin_pool_page_size(",
        "pub(super) fn parse_admin_pool_search(",
        "pub(super) fn parse_admin_pool_status_filter(",
        "pub(super) fn admin_pool_provider_id_from_path(",
        "pub(super) fn is_admin_pool_route(",
        "pub(super) fn build_admin_pool_error_response(",
    ] {
        assert!(
            pool_admin_support.contains(pattern),
            "handlers/admin/provider/pool_admin/support.rs should own {pattern}"
        );
    }

    let pool_admin_read_routes = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/pool_admin/read_routes.rs",
    );
    assert!(
        pool_admin_read_routes.contains("use super::support::{"),
        "handlers/admin/provider/pool_admin/read_routes.rs should import parsing helpers from support.rs"
    );

    let pool_admin_batch_routes = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/pool_admin/batch_routes.rs",
    );
    assert!(
        pool_admin_batch_routes.contains("use super::support::{"),
        "handlers/admin/provider/pool_admin/batch_routes.rs should import shared route helpers from support.rs"
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

    let write_keys =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/write/keys.rs");
    assert!(
        write_keys.contains("use super::normalize::{normalize_auth_type, validate_vertex_api_formats};"),
        "handlers/admin/provider/write/keys.rs should import write-local normalize helpers from write::normalize"
    );
    assert!(
        write_keys.contains("normalize_json_object,")
            && write_keys.contains("normalize_string_list,"),
        "handlers/admin/provider/write/keys.rs should import JSON normalization helpers from admin::shared directly"
    );

    let write_provider =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/write/provider.rs");
    assert!(
        write_provider.contains("use super::normalize::normalize_provider_type_input;"),
        "handlers/admin/provider/write/provider.rs should import provider-type normalization from write::normalize"
    );
    assert!(
        write_provider.contains("crate::handlers::admin::shared::normalize_json_object;"),
        "handlers/admin/provider/write/provider.rs should import JSON normalization from admin::shared directly"
    );

    let endpoint_keys_reads = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/endpoint_keys/reads.rs",
    );
    assert!(
        endpoint_keys_reads.contains("use super::super::write::reveal::{"),
        "handlers/admin/provider/endpoint_keys/reads.rs should import explicit reveal owner from write::reveal"
    );
    let endpoint_keys_mutations = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/endpoint_keys/mutations.rs",
    );
    for pattern in ["use super::super::write::keys::{"] {
        assert!(
            endpoint_keys_mutations.contains(pattern),
            "handlers/admin/provider/endpoint_keys/mutations.rs should import explicit write owner {pattern}"
        );
    }

    let crud_routes =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/crud/routes.rs");
    assert!(
        crud_routes.contains("crate::handlers::admin::provider::write::provider::{"),
        "handlers/admin/provider/crud/routes.rs should import provider record builders from write::provider"
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
        "pub(super) struct AdminProviderOpsCheckinOutcome",
    ] {
        assert!(
            providers_support.contains(pattern),
            "handlers/admin/provider/ops/providers/support.rs should own {pattern}"
        );
    }

    let providers_routes = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/ops/providers/routes.rs",
    );
    assert!(
        providers_routes.contains("use super::actions::{"),
        "handlers/admin/provider/ops/providers/routes.rs should import action helpers from actions.rs"
    );
    assert!(
        providers_routes.contains("use super::config::{"),
        "handlers/admin/provider/ops/providers/routes.rs should import config helpers from config.rs"
    );
    assert!(
        providers_routes.contains("use super::support::{"),
        "handlers/admin/provider/ops/providers/routes.rs should import DTO/constants from support.rs"
    );
    assert!(
        providers_routes.contains("use super::verify::{"),
        "handlers/admin/provider/ops/providers/routes.rs should import verify helpers from verify.rs"
    );
    assert!(
        !providers_routes.contains("use super::{"),
        "handlers/admin/provider/ops/providers/routes.rs should not depend on a catch-all providers::mod hub"
    );

    for path in [
        "apps/aether-gateway/src/maintenance/runtime.rs",
        "apps/aether-gateway/src/maintenance/runtime/provider_checkin.rs",
    ] {
        let contents = read_workspace_file(path);
        assert!(
            contents.contains(
                "provider::ops::providers::actions::admin_provider_ops_local_action_response"
            ),
            "{path} should call provider ops action helper through providers::actions"
        );
        assert!(
            !contents
                .contains("provider::ops::providers::admin_provider_ops_local_action_response"),
            "{path} should not depend on providers::mod action helper re-export"
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

    let oauth_refresh =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/oauth/refresh.rs");
    for pattern in [
        "use super::quota::antigravity::refresh_antigravity_provider_quota_locally;",
        "use super::quota::codex::refresh_codex_provider_quota_locally;",
        "use super::quota::kiro::refresh_kiro_provider_quota_locally;",
        "use super::quota::shared::persist_provider_quota_refresh_state;",
    ] {
        assert!(
            oauth_refresh.contains(pattern),
            "handlers/admin/provider/oauth/refresh.rs should import quota helper via explicit owner {pattern}"
        );
    }

    let quota_codex =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/oauth/quota/codex.rs");
    assert!(
        quota_codex.contains("use super::shared::{"),
        "handlers/admin/provider/oauth/quota/codex.rs should import common quota helpers from shared.rs"
    );
    let quota_kiro =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/oauth/quota/kiro.rs");
    assert!(
        quota_kiro.contains("use super::shared::{"),
        "handlers/admin/provider/oauth/quota/kiro.rs should import common quota helpers from shared.rs"
    );
    let quota_antigravity = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/quota/antigravity.rs",
    );
    assert!(
        quota_antigravity.contains("use super::shared::{"),
        "handlers/admin/provider/oauth/quota/antigravity.rs should import common quota helpers from shared.rs"
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
        "apps/aether-gateway/src/handlers/admin/provider/endpoint_keys/mutations.rs",
    );
    for pattern in [
        "update_key",
        "delete_key",
        "batch_delete_keys",
        "clear_oauth_invalid",
        "create_provider_key",
    ] {
        assert!(
            endpoint_keys_mutations.contains(pattern),
            "handlers/admin/provider/endpoint_keys/mutations.rs should own mutation route {pattern}"
        );
    }

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
    for pattern in ["mod payloads;", "mod write;"] {
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
