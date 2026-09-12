use std::convert::Infallible;
use std::sync::{Arc, Mutex};

use aether_data::repository::proxy_nodes::{InMemoryProxyNodeRepository, StoredProxyNode};
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider;
use axum::body::{to_bytes, Body, Bytes};
use axum::response::Response;
use axum::routing::any;
use axum::{extract::Request, Json, Router};
use http::header::{HeaderName, HeaderValue};
use http::StatusCode;
use serde_json::json;

use crate::constants::{
    CONTROL_EXECUTED_HEADER, CONTROL_EXECUTE_FALLBACK_HEADER, DEPENDENCY_REASON_HEADER,
    EXECUTION_PATH_EXECUTION_RUNTIME_STREAM, EXECUTION_PATH_EXECUTION_RUNTIME_SYNC,
    EXECUTION_PATH_HEADER, EXECUTION_PATH_LOCAL_AI_PUBLIC,
    EXECUTION_PATH_LOCAL_EXECUTION_RUNTIME_MISS, LOCAL_EXECUTION_RUNTIME_MISS_REASON_HEADER,
    TRACE_ID_HEADER,
};

use super::{
    build_router, build_router_with_execution_runtime_override, build_router_with_state,
    build_state_with_execution_runtime_override, next_non_keepalive_chunk, start_server,
    strip_sse_keepalive_comments, wait_until, AppState, FrontdoorCorsConfig,
    FrontdoorUserRpmConfig, GatewayFallbackMetricKind, GatewayFallbackReason, UsageRuntimeConfig,
    VideoTaskTruthSourceMode,
};

/// Build the real proxy-node repository used by execution-runtime fixtures.
///
/// Production proxy resolution deliberately fails closed when a configured
/// node id is missing or stale. These tests exercise the resolved-node path,
/// so each fixture must seed an online manual node just as deployment state
/// would. Manual nodes do not need a password when the configured URL has no
/// credentials.
pub(super) fn ai_execute_proxy_node_repository<I, S>(
    node_ids: I,
) -> Arc<InMemoryProxyNodeRepository>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let nodes = node_ids.into_iter().map(|node_id| {
        let node_id = node_id.as_ref();
        StoredProxyNode::new(
            node_id.to_string(),
            format!("ai-execute-{node_id}"),
            "127.0.0.1".to_string(),
            1,
            true,
            "online".to_string(),
            30,
            0,
            0,
            0,
            0,
            0,
            false,
            false,
            1,
        )
        .expect("ai_execute proxy node should build")
        .with_manual_proxy_fields(Some("http://127.0.0.1:1".to_string()), None, None)
        .with_tunnel_generation(format!("ai-execute-generation-{node_id}"))
    });
    Arc::new(InMemoryProxyNodeRepository::seed(nodes))
}

/// Add a test-only non-retryable status rule to a provider fixture.
///
/// Execution-runtime error fixtures represent a single upstream response. A
/// `429` is normally retryable by the production policy, so without an
/// explicit stop rule the candidate loop would consume the fixture and return
/// a synthetic 503 instead of the upstream error the test is exercising.
pub(super) fn ai_execute_provider_stop_on_status_code(
    mut provider: StoredProviderCatalogProvider,
    status_code: u16,
) -> StoredProviderCatalogProvider {
    let mut config = provider
        .config
        .take()
        .unwrap_or_else(|| serde_json::json!({}));
    let object = config
        .as_object_mut()
        .expect("provider test config should be a JSON object");
    let rules = object
        .entry("failover_rules".to_string())
        .or_insert_with(|| serde_json::json!({}));
    let rules_object = rules
        .as_object_mut()
        .expect("provider failover_rules test config should be a JSON object");
    let statuses = rules_object
        .entry("stop_on_status_codes".to_string())
        .or_insert_with(|| serde_json::json!([]));
    let status_values = statuses
        .as_array_mut()
        .expect("provider stop_on_status_codes test config should be an array");
    if !status_values
        .iter()
        .any(|value| value.as_u64() == Some(u64::from(status_code)))
    {
        status_values.push(serde_json::json!(status_code));
    }
    provider.config = Some(config);
    provider
}

mod control_execute;
mod fallback;
mod finalize_local;
mod finalize_local_cli;
mod finalize_local_provider;
mod lifecycle;
mod stream;
mod stream_cli;
mod stream_provider;
mod stream_provider_gemini;
mod sync;
