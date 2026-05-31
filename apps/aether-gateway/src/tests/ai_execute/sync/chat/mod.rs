use std::convert::Infallible;
use std::sync::{Arc, Mutex};

use axum::body::{to_bytes, Body, Bytes};
use axum::response::Response;
use axum::routing::any;
use axum::{extract::Request, Json, Router};
use http::header::{HeaderName, HeaderValue};
use http::StatusCode;
use serde_json::json;

fn run_async_test_on_large_stack<F>(name: &'static str, future: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let handle = std::thread::Builder::new()
        .name(name.to_string())
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime should build")
                .block_on(future);
        })
        .expect("large-stack sync chat test thread should spawn");

    if let Err(payload) = handle.join() {
        std::panic::resume_unwind(payload);
    }
}

macro_rules! large_stack_async_test {
    ($test_name:ident, $impl_name:ident) => {
        #[test]
        fn $test_name() {
            run_async_test_on_large_stack(stringify!($test_name), $impl_name());
        }
    };
}

use crate::constants::{
    CONTROL_EXECUTED_HEADER, CONTROL_EXECUTE_FALLBACK_HEADER, DEPENDENCY_REASON_HEADER,
    EXECUTION_PATH_EXECUTION_RUNTIME_STREAM, EXECUTION_PATH_EXECUTION_RUNTIME_SYNC,
    EXECUTION_PATH_HEADER, EXECUTION_PATH_LOCAL_EXECUTION_RUNTIME_MISS,
    LOCAL_EXECUTION_RUNTIME_MISS_REASON_HEADER, TRACE_ID_HEADER,
};

use super::{
    build_router, build_router_with_execution_runtime_override, build_router_with_state,
    build_state_with_execution_runtime_override, start_server, wait_until, AppState,
    FrontdoorCorsConfig, FrontdoorUserRpmConfig, GatewayFallbackMetricKind, GatewayFallbackReason,
    UsageRuntimeConfig, VideoTaskTruthSourceMode,
};
use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
use aether_data::repository::auth::{
    InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeySnapshot,
};
use aether_data::repository::candidate_selection::InMemoryMinimalCandidateSelectionReadRepository;
use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data_contracts::repository::candidate_selection::{
    StoredMinimalCandidateSelectionRow, StoredProviderModelMapping,
};
use aether_data_contracts::repository::candidates::{
    RequestCandidateReadRepository, RequestCandidateStatus,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use sha2::{Digest, Sha256};

mod failover;
mod local_decision;
mod pii_redaction;
