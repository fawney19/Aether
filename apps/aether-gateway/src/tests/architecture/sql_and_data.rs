use super::*;

#[test]
fn handlers_do_not_inline_sql_queries() {
    assert_no_sqlx_queries("src/handlers");
}

#[test]
fn gateway_runtime_does_not_inline_sql_queries() {
    assert_no_sqlx_queries("src/state/runtime");
}

#[test]
fn wallet_repository_does_not_reexport_settlement_types() {
    let wallet_mod = read_workspace_file("crates/aether-data/src/repository/wallet/mod.rs");
    let wallet_types = read_workspace_file("crates/aether-data/src/repository/wallet/types.rs");
    let wallet_sql = read_workspace_file("crates/aether-data/src/repository/wallet/sql.rs");
    let wallet_memory = read_workspace_file("crates/aether-data/src/repository/wallet/memory.rs");

    assert!(
        !wallet_mod.contains("StoredUsageSettlement"),
        "wallet/mod.rs should not export StoredUsageSettlement"
    );
    assert!(
        !wallet_mod.contains("UsageSettlementInput"),
        "wallet/mod.rs should not export UsageSettlementInput"
    );
    assert!(
        !wallet_types.contains("pub use crate::repository::settlement"),
        "wallet/types.rs should not re-export settlement types"
    );
    assert!(
        !wallet_types.contains("async fn settle_usage("),
        "wallet/types.rs should not own settlement entrypoints"
    );
    assert!(
        !wallet_sql.contains("impl SettlementWriteRepository"),
        "wallet/sql.rs should not implement SettlementWriteRepository"
    );
    assert!(
        !wallet_memory.contains("impl SettlementWriteRepository"),
        "wallet/memory.rs should not implement SettlementWriteRepository"
    );
}

#[test]
fn gateway_system_config_types_are_owned_by_aether_data() {
    let state_mod = read_workspace_file("apps/aether-gateway/src/data/state/mod.rs");
    assert!(
        state_mod.contains("aether_data::repository::system"),
        "data/state/mod.rs should depend on aether-data system types"
    );
    assert!(
        !state_mod.contains("pub(crate) struct StoredSystemConfigEntry"),
        "data/state/mod.rs should not define StoredSystemConfigEntry locally"
    );

    let state_core = read_workspace_file("apps/aether-gateway/src/data/state/core.rs");
    for pattern in [
        "backend.list_system_config_entries().await",
        "upsert_system_config_entry(key, value, description)",
        "AdminSystemStats::default()",
    ] {
        assert!(
            state_core.contains(pattern),
            "data/state/core.rs should use shared system DTO path {pattern}"
        );
    }
    for pattern in [
        "|(key, value, description, updated_at_unix_secs)|",
        "Ok((0, 0, 0, 0))",
    ] {
        assert!(
            !state_core.contains(pattern),
            "data/state/core.rs should not own local system DTO projection {pattern}"
        );
    }

    let system_types = read_workspace_file("crates/aether-data/src/repository/system.rs");
    for pattern in [
        "pub struct StoredSystemConfigEntry",
        "pub struct AdminSystemStats",
        "pub struct AdminSecurityBlacklistEntry",
    ] {
        assert!(
            system_types.contains(pattern),
            "aether-data system module should own {pattern}"
        );
    }

    let admin_types = read_workspace_file("apps/aether-gateway/src/state/admin_types.rs");
    assert!(
        admin_types.contains("aether_data::repository::system::AdminSecurityBlacklistEntry"),
        "state/admin_types.rs should re-export AdminSecurityBlacklistEntry from aether-data"
    );
    assert!(
        !admin_types.contains("struct AdminSecurityBlacklistEntry"),
        "state/admin_types.rs should not define AdminSecurityBlacklistEntry locally"
    );

    let runtime_mod = read_workspace_file("apps/aether-gateway/src/state/runtime/mod.rs");
    assert!(
        !runtime_mod.contains("AdminSecurityBlacklistEntryPayload"),
        "state/runtime/mod.rs should not keep the unused blacklist payload wrapper"
    );
}

#[test]
fn gateway_auth_snapshot_type_is_owned_by_aether_data() {
    let gateway_auth = read_workspace_file("apps/aether-gateway/src/data/auth.rs");
    let runtime_mod = read_workspace_file("apps/aether-gateway/src/state/runtime/mod.rs");
    let auth_api_keys =
        read_workspace_file("apps/aether-gateway/src/state/runtime/auth/api_keys.rs");
    assert!(
        gateway_auth.contains("aether_data::repository::auth"),
        "data/auth.rs should depend on aether-data auth snapshot types"
    );
    assert!(
        gateway_auth.contains("ResolvedAuthApiKeySnapshot as GatewayAuthApiKeySnapshot"),
        "data/auth.rs should expose the shared resolved auth snapshot type under the gateway-facing name"
    );
    for pattern in [
        "pub(crate) struct GatewayAuthApiKeySnapshot",
        "pub(crate) async fn read_auth_api_key_snapshot(",
        "pub(crate) async fn read_auth_api_key_snapshot_by_key_hash(",
        "fn effective_allowed_providers(",
        "fn effective_allowed_api_formats(",
        "fn effective_allowed_models(",
    ] {
        assert!(
            !gateway_auth.contains(pattern),
            "data/auth.rs should not own local auth snapshot logic {pattern}"
        );
    }
    for pattern in [
        "pub(crate) async fn read_auth_api_key_snapshot(",
        "pub(crate) async fn read_auth_api_key_snapshots_by_ids(",
    ] {
        assert!(
            !auth_api_keys.contains(pattern),
            "state/runtime/auth/api_keys.rs should not keep auth snapshot read wrapper {pattern}"
        );
    }
    assert!(
        !runtime_mod.contains("mod audit;"),
        "state/runtime/mod.rs should not keep the obsolete audit runtime module"
    );
    assert!(
        auth_api_keys.contains("touch_auth_api_key_last_used_best_effort"),
        "state/runtime/auth/api_keys.rs should own auth api key last_used touch helper"
    );
    assert!(
        !auth_api_keys.contains("fn has_auth_api_key_writer("),
        "state/runtime/auth/api_keys.rs should not keep auth api key writer passthrough"
    );

    let auth_types = read_workspace_file("crates/aether-data/src/repository/auth/types.rs");
    for pattern in [
        "pub struct ResolvedAuthApiKeySnapshot",
        "pub trait ResolvedAuthApiKeySnapshotReader",
        "pub async fn read_resolved_auth_api_key_snapshot(",
        "pub async fn read_resolved_auth_api_key_snapshot_by_key_hash(",
        "pub async fn read_resolved_auth_api_key_snapshot_by_user_api_key_ids(",
        "pub fn effective_allowed_providers(&self)",
        "pub fn effective_allowed_api_formats(&self)",
        "pub fn effective_allowed_models(&self)",
    ] {
        assert!(
            auth_types.contains(pattern),
            "aether-data auth types should own {pattern}"
        );
    }
}

#[test]
fn gateway_auth_data_layer_does_not_keep_ldap_row_wrapper() {
    let gateway_auth_state = read_workspace_file("apps/aether-gateway/src/data/state/auth.rs");
    for pattern in [
        "struct StoredLdapAuthUserRow",
        "fn map_ldap_user_auth_row(",
        "Result<Option<StoredLdapAuthUserRow>, DataLayerError>",
        "existing.user.",
    ] {
        assert!(
            !gateway_auth_state.contains(pattern),
            "data/state/auth.rs should not keep ldap row wrapper {pattern}"
        );
    }

    for pattern in [
        "Result<Option<StoredUserAuthRecord>, DataLayerError>",
        "return map_user_auth_row(row).map(Some);",
    ] {
        assert!(
            gateway_auth_state.contains(pattern),
            "data/state/auth.rs should use shared user auth record directly via {pattern}"
        );
    }
}

#[test]
fn gateway_provider_oauth_storage_types_are_owned_by_aether_data() {
    let provider_oauth_state =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/oauth/state.rs");
    assert!(
        provider_oauth_state.contains("aether_data::repository::provider_oauth"),
        "provider_oauth/state.rs should depend on aether-data provider oauth storage types"
    );
    for pattern in [
        "pub(crate) struct StoredAdminProviderOAuthDeviceSession",
        "pub(crate) struct StoredAdminProviderOAuthState",
        "const KIRO_DEVICE_AUTH_SESSION_PREFIX",
        "fn provider_oauth_device_session_key(",
        "fn build_provider_oauth_batch_task_status_payload(",
        "fn provider_oauth_batch_task_key(",
        "const PROVIDER_OAUTH_BATCH_TASK_TTL_SECS",
        "format!(\"provider_oauth_state:{nonce}\")",
    ] {
        assert!(
            !provider_oauth_state.contains(pattern),
            "provider_oauth/state.rs should not own local storage helper {pattern}"
        );
    }

    let dispatch_device = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/device.rs",
    );
    assert!(
        dispatch_device.contains("aether_data::repository::provider_oauth"),
        "provider_oauth/dispatch/device.rs should use shared provider oauth storage DTOs"
    );

    let shared_provider_oauth =
        read_workspace_file("crates/aether-data/src/repository/provider_oauth.rs");
    for pattern in [
        "pub struct StoredAdminProviderOAuthDeviceSession",
        "pub struct StoredAdminProviderOAuthState",
        "pub fn provider_oauth_device_session_storage_key(",
        "pub fn provider_oauth_state_storage_key(",
        "pub fn provider_oauth_batch_task_storage_key(",
        "pub fn build_provider_oauth_batch_task_status_payload(",
        "pub const KIRO_DEVICE_AUTH_SESSION_TTL_BUFFER_SECS: u64 = 60;",
        "pub const PROVIDER_OAUTH_BATCH_TASK_TTL_SECS: u64 = 24 * 60 * 60;",
        "pub const PROVIDER_OAUTH_STATE_TTL_SECS: u64 = 600;",
    ] {
        assert!(
            shared_provider_oauth.contains(pattern),
            "aether-data provider oauth storage module should own {pattern}"
        );
    }
}
