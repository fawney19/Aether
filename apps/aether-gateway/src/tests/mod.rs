use std::time::{SystemTime, UNIX_EPOCH};

pub(super) use std::convert::Infallible;
pub(super) use std::sync::{Arc, Mutex};

use aether_contracts::USAGE_SERVER_NOW_UNIX_MS_HEADER;
pub(super) use axum::body::{to_bytes, Body, Bytes};
pub(super) use axum::response::Response;
pub(super) use axum::routing::any;
pub(super) use axum::{extract::Request, Json, Router};
pub(super) use http::header::{HeaderName, HeaderValue};
pub(super) use http::StatusCode;
pub(super) use serde_json::json;

mod ai_execute;
mod architecture;
mod async_task;
mod audit;
mod concurrency;
mod control;
mod files;
mod frontdoor;
mod proxy;
mod usage;
mod video;

pub(super) use super::async_task::VideoTaskTruthSourceMode;
pub(super) use super::constants::*;
pub(super) use super::fallback_metrics::{GatewayFallbackMetricKind, GatewayFallbackReason};
pub(super) use super::rate_limit::FrontdoorUserRpmConfig;
pub(super) use super::router::{attach_static_frontend, build_router, build_router_with_state};
pub(super) use super::state::{AppState, FrontdoorCorsConfig};
pub(super) use super::usage::UsageRuntimeConfig;

const SERVER_NOW_HEADER_TEST_TOLERANCE_MS: u64 = 1_000;

pub(super) fn unix_epoch_millis_for_tests() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("current time should be after epoch")
        .as_millis()
        .try_into()
        .expect("current epoch millis should fit in u64")
}

pub(super) fn assert_usage_server_now_header_between(
    headers: &reqwest::header::HeaderMap,
    lower_bound_unix_ms: u64,
    upper_bound_unix_ms: u64,
) {
    let server_now_unix_ms = headers
        .get(USAGE_SERVER_NOW_UNIX_MS_HEADER)
        .expect("usage response should include server timing header")
        .to_str()
        .expect("server timing header should be valid ASCII")
        .parse::<u64>()
        .expect("server timing header should be epoch millis");

    assert!(
        server_now_unix_ms >= lower_bound_unix_ms.saturating_sub(SERVER_NOW_HEADER_TEST_TOLERANCE_MS)
            && server_now_unix_ms
                <= upper_bound_unix_ms.saturating_add(SERVER_NOW_HEADER_TEST_TOLERANCE_MS),
        "server timing header {server_now_unix_ms} should be near request window {lower_bound_unix_ms}..={upper_bound_unix_ms}"
    );
}

pub(super) async fn start_server(app: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = crate::test_support::bind_loopback_listener()
        .await
        .expect("listener should bind");
    let addr = listener.local_addr().expect("local addr should resolve");
    let handle = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .expect("server should run");
    });
    (format!("http://{addr}"), handle)
}

pub(super) async fn send_request(app: Router, mut request: Request) -> Response {
    use tower::ServiceExt;

    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            40000,
        ))));
    app.oneshot(request)
        .await
        .expect("router request should complete")
}

pub(super) fn build_router_with_execution_runtime_override(
    execution_runtime_override_base_url: impl Into<String>,
) -> Router {
    let state = build_state_with_execution_runtime_override(execution_runtime_override_base_url);
    build_router_with_state(state)
}

pub(super) fn build_state_with_execution_runtime_override(
    execution_runtime_override_base_url: impl Into<String>,
) -> AppState {
    AppState::new()
        .expect("gateway should build")
        .with_execution_runtime_override_base_url(execution_runtime_override_base_url)
}

pub(super) async fn wait_until(timeout_ms: u64, mut predicate: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    loop {
        if predicate() {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "condition not met within {}ms",
            timeout_ms
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

pub(crate) fn strip_sse_keepalive_comments(body: &str) -> String {
    body.replace(": aether-keepalive\n\n", "")
}

pub(crate) async fn next_non_keepalive_chunk(response: &mut reqwest::Response) -> Bytes {
    loop {
        let chunk = response
            .chunk()
            .await
            .expect("chunk should read")
            .expect("chunk should exist");
        if chunk.as_ref() != b": aether-keepalive\n\n" {
            return chunk;
        }
    }
}
