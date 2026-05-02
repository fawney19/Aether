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
mod api;
mod async_task;
mod audit;
mod auth;
mod cache;
mod clock;
mod constants;
mod control;
mod data;
mod error;
mod execution_runtime;
mod executor;
mod fallback_metrics;
mod frontdoor_loop_guard;
mod handlers;
mod headers;
mod hooks;
mod log_ids;
mod maintenance;
pub(crate) mod middleware;
mod model_fetch;
mod oauth;
mod orchestration;
mod provider_key_auth;
pub(crate) use aether_provider_transport as provider_transport;
mod query;
mod rate_limit;
mod request_candidate_runtime;
mod router;
mod scheduler;
mod state;
mod tunnel;
mod usage;
mod video_tasks;
mod wallet_runtime;

pub(crate) use self::ai_serving::api::{
    AiControlPlanRequest, EXECUTION_RUNTIME_STREAM_DECISION_ACTION,
    EXECUTION_RUNTIME_SYNC_DECISION_ACTION, GEMINI_FILES_DOWNLOAD_PLAN_KIND,
    OPENAI_VIDEO_CONTENT_PLAN_KIND,
};
pub(crate) use self::ai_serving::{
    AiExecutionDecision, AiExecutionPlanPayload, AiStreamAttempt, AiSyncAttempt,
};
pub use self::async_task::VideoTaskTruthSourceMode;
pub use self::data::GatewayDataConfig;
pub(crate) use self::error::GatewayError;
pub(crate) use self::execution_runtime::{
    append_execution_contract_fields_to_value, append_local_failover_policy_to_value,
    MAX_ERROR_BODY_BYTES, MAX_STREAM_PREFETCH_FRAMES,
};
pub use self::execution_runtime::{
    build_execution_runtime_router, build_execution_runtime_router_with_request_concurrency_limit,
    build_execution_runtime_router_with_request_gates, serve_execution_runtime_tcp,
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
    let name = HeaderName::from_static(key);
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
