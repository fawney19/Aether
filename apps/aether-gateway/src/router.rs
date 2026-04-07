use std::path::PathBuf;

use axum::extract::Request;
use axum::http::Method;
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use tower::ServiceExt;
use tower_http::services::{ServeDir, ServeFile};
use tracing::warn;

use aether_runtime::{prometheus_response, ConcurrencyError, DistributedConcurrencyError};

use super::{api, handlers::proxy::proxy_request, middleware, state::AppState};

pub fn build_router() -> Result<Router, reqwest::Error> {
    Ok(build_router_with_state(AppState::new()?))
}

#[derive(Clone, Debug)]
struct FrontendStaticState {
    static_dir: PathBuf,
    index_html: PathBuf,
}

pub fn build_router_with_state(state: AppState) -> Router {
    let cors_state = state.clone();
    let mut router = Router::<AppState>::new();
    router = api::mount_core_routes(router);
    router = api::mount_operational_routes(router);
    router = api::mount_ai_routes(router);
    router = api::mount_public_support_routes(router);
    router = api::mount_oauth_routes(router);
    router = api::mount_internal_routes(router);
    router = api::mount_admin_routes(router);
    let mut router = router
        .route("/{*path}", any(proxy_request))
        .layer(axum::middleware::from_fn(middleware::access_log_middleware))
        .with_state(state);
    if cors_state.frontdoor_cors().is_some() {
        router = router.layer(axum::middleware::from_fn_with_state(
            cors_state,
            middleware::frontdoor_cors_middleware,
        ));
    }
    router
}

pub fn attach_static_frontend(router: Router, static_dir: impl Into<PathBuf>) -> Router {
    let static_dir = static_dir.into();
    let index_html = static_dir.join("index.html");
    router.layer(axum::middleware::from_fn_with_state(
        FrontendStaticState {
            static_dir,
            index_html,
        },
        frontend_static_middleware,
    ))
}

async fn frontend_static_middleware(
    axum::extract::State(frontend): axum::extract::State<FrontendStaticState>,
    request: Request,
    next: axum::middleware::Next,
) -> Response {
    let path = request.uri().path().to_string();
    if !matches!(request.method(), &Method::GET | &Method::HEAD)
        || frontend_path_bypasses_static(&path)
    {
        return next.run(request).await;
    }

    if frontend_path_targets_static_asset(&path) {
        return serve_static_asset(&frontend.static_dir, request).await;
    }

    serve_frontend_index(&frontend.index_html, request).await
}

fn frontend_path_bypasses_static(path: &str) -> bool {
    matches!(
        path,
        "/health" | "/test-connection" | crate::constants::READYZ_PATH
    ) || path.starts_with("/api/")
        || path.starts_with("/v1/")
        || path.starts_with("/v1beta/")
        || path.starts_with("/upload/")
        || path.starts_with("/_gateway/")
        || path.starts_with("/.well-known/")
}

fn frontend_path_targets_static_asset(path: &str) -> bool {
    path.rsplit('/')
        .next()
        .is_some_and(|segment| !segment.is_empty() && segment.contains('.'))
}

async fn serve_static_asset(static_dir: &PathBuf, request: Request) -> Response {
    match ServeDir::new(static_dir).oneshot(request).await {
        Ok(response) => response.into_response(),
        Err(err) => {
            warn!(error = %err, "failed to serve frontend static asset");
            axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn serve_frontend_index(index_html: &PathBuf, request: Request) -> Response {
    match ServeFile::new(index_html).oneshot(request).await {
        Ok(response) => response.into_response(),
        Err(err) => {
            warn!(error = %err, "failed to serve frontend index");
            axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub(crate) async fn metrics(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> impl axum::response::IntoResponse {
    prometheus_response(&state.metric_samples().await)
}

#[derive(Debug)]
pub(crate) enum RequestAdmissionError {
    Local(ConcurrencyError),
    Distributed(DistributedConcurrencyError),
}

pub async fn serve_tcp(bind: &str) -> Result<(), Box<dyn std::error::Error>> {
    let listener = tokio::net::TcpListener::bind(bind).await?;
    let router = build_router()?;
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}
