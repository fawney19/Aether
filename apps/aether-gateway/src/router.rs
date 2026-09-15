use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::Request;
use axum::http::header::{CACHE_CONTROL, EXPIRES, PRAGMA};
use axum::http::{HeaderValue, Method};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto::Builder as HyperServerBuilder;
use hyper_util::service::TowerToHyperService;
use tower::{Service as _, ServiceExt};
use tower_http::services::{ServeDir, ServeFile};
use tracing::warn;

use aether_gateway_frontdoor::{http_connection_limit, HttpConnectionBudget};
use aether_runtime::{prometheus_response, ConcurrencyError};
use aether_runtime_state::RuntimeSemaphoreError;

use super::{api, handlers::proxy::proxy_request, middleware, state::AppState};

// Keep the compatibility `serve_tcp` entry point subject to the same parser
// protections as the configured binary listener. These are metadata limits;
// request and response bodies remain streaming after the first request gate
// opens. The HTTP/2 stream default intentionally stays high for capable hosts.
const DEFAULT_TCP_HTTP2_MAX_CONCURRENT_STREAMS: u32 = 16_384;
const DEFAULT_TCP_HTTP_HEADER_READ_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_TCP_HTTP_HEADER_MAX_BYTES: usize = 64 * 1024;
const DEFAULT_TCP_HTTP_MAX_HEADERS: usize = 256;

#[derive(Clone)]
struct FirstRequestGate {
    seen: Arc<AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
}

impl FirstRequestGate {
    fn new() -> Self {
        Self {
            seen: Arc::new(AtomicBool::new(false)),
            notify: Arc::new(tokio::sync::Notify::new()),
        }
    }

    fn mark_seen(&self) {
        if !self.seen.swap(true, Ordering::Release) {
            self.notify.notify_one();
        }
    }

    fn is_seen(&self) -> bool {
        self.seen.load(Ordering::Acquire)
    }
}

#[derive(Clone)]
struct FirstRequestService<S> {
    inner: S,
    gate: FirstRequestGate,
}

impl<S, Req> tower::Service<Req> for FirstRequestService<S>
where
    S: tower::Service<Req>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Req) -> Self::Future {
        self.gate.mark_seen();
        self.inner.call(request)
    }
}

async fn drive_first_request_gate<F, E>(
    connection: F,
    gate: FirstRequestGate,
    timeout: Duration,
) -> Result<(), E>
where
    F: std::future::Future<Output = Result<(), E>>,
{
    if gate.is_seen() {
        return connection.await;
    }

    let mut connection = Box::pin(connection);
    let timeout = tokio::time::sleep(timeout);
    tokio::pin!(timeout);
    let notified = gate.notify.notified();
    tokio::pin!(notified);

    tokio::select! {
        result = &mut connection => result,
        _ = &mut timeout => {
            if gate.is_seen() {
                (&mut connection).await
            } else {
                Ok(())
            }
        }
        _ = &mut notified => (&mut connection).await,
    }
}

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
    router = api::mount_operational_routes(router, state.clone());
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
    middleware::apply_cf_header_stripping(router)
}

pub fn attach_static_frontend(router: Router, static_dir: impl Into<PathBuf>) -> Router {
    let static_dir = static_dir.into();
    let index_html = static_dir.join("index.html");
    middleware::apply_cf_header_stripping(router.layer(axum::middleware::from_fn_with_state(
        FrontendStaticState {
            static_dir,
            index_html,
        },
        frontend_static_middleware,
    )))
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
        || path == "/openai/v1/videos"
        || path.starts_with("/openai/v1/videos/")
        || path.starts_with("/v1beta/")
        || path.starts_with("/upload/")
        || path.starts_with("/_gateway/")
        || path.starts_with("/.well-known/")
        || path.starts_with("/install/")
        || path.starts_with("/install-tunnel/")
        || path.starts_with("/i/")
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
        Ok(mut response) => {
            let headers = response.headers_mut();
            headers.insert(
                CACHE_CONTROL,
                HeaderValue::from_static("no-store, no-cache, must-revalidate"),
            );
            headers.insert(PRAGMA, HeaderValue::from_static("no-cache"));
            headers.insert(EXPIRES, HeaderValue::from_static("0"));
            response.into_response()
        }
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
    Distributed(RuntimeSemaphoreError),
}

pub async fn serve_tcp(bind: &str) -> Result<(), Box<dyn std::error::Error>> {
    let listener = tokio::net::TcpListener::bind(bind).await?;
    let router = build_router()?;
    let configured_connection_limit = std::env::var("AETHER_GATEWAY_MAX_HTTP_CONNECTIONS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok());
    // This compatibility entry point has no configured request capacities or FD probe.
    let connection_budget = Arc::new(HttpConnectionBudget::new(http_connection_limit(
        configured_connection_limit,
        2048,
        2048,
        None,
    )));
    let mut make_service = router.into_make_service_with_connect_info::<std::net::SocketAddr>();
    loop {
        let (io, remote_addr) = connection_budget.accept(&listener).await;
        let Ok(io) = connection_budget.try_admit(io) else {
            tokio::task::yield_now().await;
            continue;
        };
        let tower_service = make_service
            .call(remote_addr)
            .await
            .unwrap_or_else(|err| match err {})
            .map_request(|request: http::Request<Incoming>| request.map(Body::new));
        let first_request_gate = FirstRequestGate::new();
        let hyper_service = TowerToHyperService::new(FirstRequestService {
            inner: tower_service,
            gate: first_request_gate.clone(),
        });
        let io = TokioIo::new(io);

        tokio::spawn(async move {
            let mut builder = HyperServerBuilder::new(TokioExecutor::new());
            builder
                .http1()
                .timer(TokioTimer::new())
                .header_read_timeout(DEFAULT_TCP_HTTP_HEADER_READ_TIMEOUT)
                .max_buf_size(DEFAULT_TCP_HTTP_HEADER_MAX_BYTES)
                .max_headers(DEFAULT_TCP_HTTP_MAX_HEADERS);
            builder
                .http2()
                .timer(TokioTimer::new())
                .enable_connect_protocol()
                .max_concurrent_streams(DEFAULT_TCP_HTTP2_MAX_CONCURRENT_STREAMS)
                .max_header_list_size(DEFAULT_TCP_HTTP_HEADER_MAX_BYTES as u32);

            let result = drive_first_request_gate(
                builder.serve_connection_with_upgrades(io, hyper_service),
                first_request_gate,
                DEFAULT_TCP_HTTP_HEADER_READ_TIMEOUT,
            )
            .await;
            if let Err(error) = result {
                tracing::trace!(error = ?error, "compatibility gateway connection closed with error");
            }
        });
    }
}
