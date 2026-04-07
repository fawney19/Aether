pub(super) use std::convert::Infallible;
pub(super) use std::sync::{Arc, Mutex};

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

pub(super) async fn start_server(app: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
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
