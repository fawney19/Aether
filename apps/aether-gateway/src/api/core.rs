use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Json;
use axum::Router;
use serde_json::json;

use crate::constants::{
    FRONTDOOR_MANIFEST_PATH, FRONTDOOR_MANIFEST_VERSION, INTERNAL_FRONTDOOR_MANIFEST_PATH,
    INTERNAL_GATEWAY_PATH_PREFIXES, INTERNAL_GATEWAY_ROUTE_GROUPS, READYZ_PATH,
    RUST_FRONTDOOR_OWNED_ROUTE_PATTERNS,
};
use crate::AppState;

pub(crate) fn mount_core_routes(router: Router<AppState>) -> Router<AppState> {
    router
        .route(FRONTDOOR_MANIFEST_PATH, get(frontdoor_manifest))
        .route(INTERNAL_FRONTDOOR_MANIFEST_PATH, get(frontdoor_manifest))
        .route(READYZ_PATH, get(readyz))
        .route("/_gateway/health", get(health))
}

fn current_gateway_version() -> &'static str {
    option_env!("AETHER_BUILD_VERSION")
        .filter(|version| !version.is_empty())
        .unwrap_or(env!("CARGO_PKG_VERSION"))
}

pub(crate) async fn health(State(state): State<AppState>) -> impl IntoResponse {
    let request_concurrency = state.request_concurrency_snapshot().map(|snapshot| {
        json!({
            "limit": snapshot.limit,
            "in_flight": snapshot.in_flight,
            "available_permits": snapshot.available_permits,
            "high_watermark": snapshot.high_watermark,
            "rejected": snapshot.rejected,
        })
    });
    let distributed_request_concurrency = state
        .distributed_request_concurrency_snapshot()
        .await
        .ok()
        .flatten()
        .map(|snapshot| {
            json!({
                "limit": snapshot.limit,
                "in_flight": snapshot.in_flight,
                "available_permits": snapshot.available_permits,
                "high_watermark": snapshot.high_watermark,
                "rejected": snapshot.rejected,
            })
        });
    Json(json!({
        "status": "ok",
        "component": "aether-gateway",
        "control_api_enabled": true,
        "request_concurrency": request_concurrency,
        "distributed_request_concurrency": distributed_request_concurrency,
    }))
}

pub(crate) async fn readyz(State(_state): State<AppState>) -> impl IntoResponse {
    Json(json!({
        "status": "ready",
        "component": "aether-gateway",
        "manifest_version": FRONTDOOR_MANIFEST_VERSION,
        "manifest_path": FRONTDOOR_MANIFEST_PATH,
        "warmup_status": "disabled",
        "gate_readiness": false,
    }))
}

pub(crate) async fn frontdoor_manifest(State(state): State<AppState>) -> impl IntoResponse {
    let frontdoor_cors = state.frontdoor_cors();
    let cors_enabled = frontdoor_cors.is_some();
    let cors_allow_credentials = frontdoor_cors
        .as_ref()
        .map(|config| config.allow_credentials())
        .unwrap_or(false);
    let cors_allowed_origins = frontdoor_cors
        .as_ref()
        .map(|config| config.allowed_origins().to_vec())
        .unwrap_or_default();
    Json(json!({
        "component": "aether-gateway",
        "manifest_version": FRONTDOOR_MANIFEST_VERSION,
        "version": current_gateway_version(),
        "mode": "compatibility_frontdoor",
        "entrypoints": {
            "public_manifest": FRONTDOOR_MANIFEST_PATH,
            "operational_manifest": INTERNAL_FRONTDOOR_MANIFEST_PATH,
            "readiness": READYZ_PATH,
            "health": "/_gateway/health",
            "metrics": "/_gateway/metrics",
        },
        "rust_frontdoor": {
            "owned_route_patterns": RUST_FRONTDOOR_OWNED_ROUTE_PATTERNS,
            "capabilities": {
                "public_proxy_catch_all": true,
                "admin_proxy_passthrough": true,
                "internal_proxy_passthrough": true,
                "trace_id_injection": true,
                "compatibility_proxy": true,
            },
            "internal_gateway": {
                "route_groups": INTERNAL_GATEWAY_ROUTE_GROUPS,
                "path_prefixes": INTERNAL_GATEWAY_PATH_PREFIXES,
                "status": "rust_native_control_plane",
            },
        },
        "features": {
            "control_api_configured": true,
            "execution_runtime_configured": state.execution_runtime_configured(),
            "request_concurrency_enabled": state.request_concurrency_snapshot().is_some(),
            "distributed_request_concurrency_enabled": state.distributed_request_gate.is_some(),
            "frontdoor_cors_enabled": cors_enabled,
            "frontdoor_cors_allow_credentials": cors_allow_credentials,
            "frontdoor_cors_allowed_origins": cors_allowed_origins,
        },
    }))
}
