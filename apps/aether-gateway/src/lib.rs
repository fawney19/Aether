#![recursion_limit = "256"]
#![allow(
    dead_code,
    unused_assignments,
    unused_imports,
    unused_mut,
    unused_variables,
    clippy::bool_assert_comparison,
    clippy::collapsible_if,
    clippy::empty_line_after_outer_attr,
    clippy::field_reassign_with_default,
    clippy::if_same_then_else,
    clippy::large_enum_variant,
    clippy::manual_div_ceil,
    clippy::manual_find,
    clippy::match_like_matches_macro,
    clippy::needless_as_bytes,
    clippy::needless_lifetimes,
    clippy::nonminimal_bool,
    clippy::question_mark,
    clippy::redundant_closure,
    clippy::result_large_err,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::useless_concat
)]

mod admin_api;
mod ai_serving;
mod allocator_metrics;
mod api;
mod async_task;
mod audit;
mod auth;
mod backup;
mod bark_push;
mod cache;
mod client_session_affinity;
mod clock;
mod constants;
mod control;
mod data;
mod dispatch;
mod email_delivery;
mod error;
mod execution_runtime;
mod executor;
mod fallback_metrics;
mod frontdoor_loop_guard;
mod handlers;
mod headers;
mod hooks;
mod image_capabilities;
mod important_notification;
mod internal_gateway_auth;
mod local_auth_token;
mod log_ids;
mod maintenance;
mod management_token_auth;
pub(crate) mod middleware;
mod model_fetch;
mod oauth;
mod orchestration;
mod plan_usage_policy;
mod privacy;
mod process_metrics;
mod provider_key_auth;
mod provider_pool_demand;
pub(crate) use aether_provider_transport as provider_transport;
mod rate_limit;
mod request_candidate_queue;
mod request_candidate_runtime;
mod request_diagnostics;
mod request_lifecycle;
mod roles;
mod router;
mod routing;
mod scheduler;
mod server_chan_push;
mod stage_metrics;
mod state;
mod system_features;
mod task_runtime;
#[cfg(feature = "testkit")]
pub mod testkit;
mod tokio_metrics;
mod tunnel;
mod upstream_admission;
mod usage;
mod video_tasks;
mod wallet_runtime;

pub(crate) use self::ai_serving::api::{
    AiControlPlanRequest, EXECUTION_RUNTIME_STREAM_DECISION_ACTION,
    EXECUTION_RUNTIME_SYNC_DECISION_ACTION, GEMINI_FILES_DOWNLOAD_PLAN_KIND,
    OPENAI_VIDEO_CONTENT_PLAN_KIND,
};
pub use self::ai_serving::api::{CODEX_CLIENT_ORIGINATOR, CODEX_CLIENT_USER_AGENT};
pub(crate) use self::ai_serving::{
    AiExecutionDecision, AiExecutionPlanPayload, AiStreamAttempt, AiSyncAttempt,
};
pub use self::async_task::VideoTaskTruthSourceMode;
pub use self::backup::{
    apply_restored_backup, restore_backup_json, BackupApplyError, BackupDecryptionKey,
    BackupRestoreError, BackupRestoreLimits, BackupRestoreScope, RestoredBackupJson,
    DEFAULT_BACKUP_MAX_ENCRYPTED_BYTES, DEFAULT_BACKUP_MAX_JSON_BYTES,
};
pub use self::data::GatewayDataConfig;
pub(crate) use self::error::GatewayError;
pub(crate) use self::execution_runtime::{
    append_execution_contract_fields_to_value, append_local_failover_policy_to_value,
    MAX_ERROR_BODY_BYTES, MAX_STREAM_PREFETCH_FRAMES,
};
pub use self::execution_runtime::{
    build_execution_runtime_router, build_execution_runtime_router_with_request_concurrency_limit,
    build_execution_runtime_router_with_request_gates,
    prewarm_direct_h2c_sender_cache_from_env_for_startup, serve_execution_runtime_tcp,
    serve_execution_runtime_unix,
};
pub(crate) use self::fallback_metrics::{GatewayFallbackMetricKind, GatewayFallbackReason};
pub use self::frontdoor_loop_guard::set_gateway_frontdoor_app_port;
pub use self::middleware::strip_cf_headers_middleware;
pub use self::rate_limit::FrontdoorUserRpmConfig;
pub(crate) use self::rate_limit::FrontdoorUserRpmOutcome;
pub use self::router::{attach_static_frontend, build_router, build_router_with_state, serve_tcp};
pub(crate) use self::state::{
    AdminBillingCollectorRecord, AdminBillingCollectorWriteInput, AdminBillingRuleRecord,
    AdminBillingRuleWriteInput, AdminWalletMutationOutcome, AdminWalletPaymentOrderRecord,
    AdminWalletRefundRecord, AdminWalletTransactionRecord, GatewayAdminPaymentCallbackView,
    GatewayUserPreferenceView, GatewayUserSessionView, LocalExecutionRuntimeMissDiagnostic,
    LocalMutationOutcome, LocalProviderDeleteTaskState,
};
pub use self::state::{AppState, FrontdoorCorsConfig};
pub use self::tunnel::{
    build_tunnel_runtime_router_with_state, tunnel_protocol, TunnelConnConfig,
    TunnelControlPlaneClient, TunnelRuntimeState,
};
#[cfg(feature = "testkit")]
pub fn configure_test_tunnel_security(
    state: &mut AppState,
    node_id: &str,
    tunnel_generation: &str,
    key: &str,
) {
    use aether_data::repository::proxy_nodes::{InMemoryProxyNodeRepository, StoredProxyNode};
    use std::sync::Arc;

    let node = StoredProxyNode::new(
        node_id.to_string(),
        "test tunnel node".to_string(),
        "127.0.0.1".to_string(),
        0,
        false,
        "offline".to_string(),
        30,
        0,
        0,
        0,
        0,
        0,
        true,
        false,
        0,
    )
    .expect("test tunnel node should be valid")
    .with_runtime_fields(
        None,
        None,
        None,
        None,
        Some(serde_json::json!({
            "tunnel_security": {"encryption_key": key}
        })),
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .with_tunnel_generation(tunnel_generation.to_string());
    let repository = Arc::new(InMemoryProxyNodeRepository::seed([node]));
    let data = self::data::GatewayDataState::with_proxy_node_repository_for_testkit(
        repository,
        aether_crypto::DEVELOPMENT_ENCRYPTION_KEY,
    );
    state.replace_data_state(Arc::new(data));
}
#[cfg(feature = "testkit")]
pub fn configure_test_tunnel_runtime_auth(
    state: TunnelRuntimeState,
    node_id: &str,
    tunnel_generation: &str,
    raw_management_token: &str,
) -> Result<TunnelRuntimeState, String> {
    use std::sync::Arc;

    let data = self::data::GatewayDataState::with_tunnel_management_auth_for_testkit(
        node_id,
        tunnel_generation,
        raw_management_token,
        aether_crypto::DEVELOPMENT_ENCRYPTION_KEY,
    )
    .map_err(|error| format!("failed to configure tunnel harness authentication: {error}"))?;
    Ok(state.with_data(Arc::new(data)))
}
pub use self::usage::UsageRuntimeConfig;

use axum::http::header::{HeaderName, HeaderValue};

fn insert_header_if_missing(
    headers: &mut http::HeaderMap,
    key: &'static str,
    value: &str,
) -> Result<(), GatewayError> {
    if headers.contains_key(key) {
        return Ok(());
    }
    let name = HeaderName::from_bytes(key.as_bytes())
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let value =
        HeaderValue::from_str(value).map_err(|err| GatewayError::Internal(err.to_string()))?;
    headers.insert(name, value);
    Ok(())
}

#[cfg(test)]
#[path = "execution_runtime/tests.rs"]
mod execution_runtime_contract_tests;

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
mod tests;
