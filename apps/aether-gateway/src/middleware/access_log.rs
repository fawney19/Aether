use std::time::Instant;

use axum::body::Body;
use axum::extract::Request;
use axum::http::header::{HeaderName, HeaderValue};
use axum::http::Method;
use axum::middleware::Next;
use axum::response::Response;
use tracing::{info, trace, warn};

use crate::constants::{
    CONTROL_REQUEST_ID_HEADER, CONTROL_ROUTE_CLASS_HEADER, EXECUTION_PATH_HEADER, TRACE_ID_HEADER,
};
use crate::headers::extract_or_generate_trace_id;
use crate::log_ids::short_request_id;

#[derive(Debug, Clone, Copy)]
pub(crate) struct RequestLogEmitted;

fn is_usage_detail_path(path: &str) -> bool {
    let Some(detail_id) = path.strip_prefix("/api/admin/usage/") else {
        return false;
    };
    !detail_id.is_empty()
        && !detail_id.contains('/')
        && !matches!(detail_id, "active" | "records" | "stats" | "heatmap")
}

pub(crate) fn should_downgrade_access_log(method: &Method, path: &str) -> bool {
    if method != Method::GET {
        return false;
    }
    let normalized_path = path.split('?').next().unwrap_or(path);
    matches!(
        normalized_path,
        "/api/admin/usage/active"
            | "/api/users/me/usage/active"
            | "/api/admin/usage/records"
            | "/api/admin/usage/stats"
            | "/api/admin/usage/aggregation/stats"
            | "/api/admin/usage/heatmap"
            | "/api/admin/usage/cache-affinity/interval-timeline"
            | "/api/admin/usage/cache-affinity/ttl-analysis"
            | "/api/admin/usage/cache-affinity/hit-analysis"
            | "/api/admin/users"
            | "/api/admin/monitoring/cache/stats"
            | "/api/admin/monitoring/cache/model-mapping/stats"
            | "/api/admin/monitoring/cache/config"
            | "/api/admin/monitoring/cache/redis-keys"
            | "/api/admin/monitoring/cache/affinities"
    ) || is_usage_detail_path(normalized_path)
        || normalized_path.starts_with("/api/admin/monitoring/trace/")
}

pub(crate) async fn access_log_middleware(mut request: Request<Body>, next: Next) -> Response {
    let started_at = Instant::now();
    let method = request.method().clone();
    let path = request
        .uri()
        .path_and_query()
        .map(|value| value.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());
    let trace_id = extract_or_generate_trace_id(request.headers());
    if !request.headers().contains_key(TRACE_ID_HEADER) {
        request.headers_mut().insert(
            HeaderName::from_static(TRACE_ID_HEADER),
            HeaderValue::from_str(&trace_id).expect("trace id should be a valid header value"),
        );
    }
    trace!(
        event_name = "http_request_started",
        log_type = "access",
        status = "started",
        trace_id = %trace_id,
        request_id = "-",
        method = %method,
        path = %path,
        route_class = "pending",
        execution_path = "pending",
        "gateway request started"
    );
    let mut response = next.run(request).await;
    if !response.headers().contains_key(TRACE_ID_HEADER) {
        response.headers_mut().insert(
            HeaderName::from_static(TRACE_ID_HEADER),
            HeaderValue::from_str(&trace_id).expect("trace id should be a valid header value"),
        );
    }
    if response.extensions().get::<RequestLogEmitted>().is_none() {
        let route_class = response
            .headers()
            .get(CONTROL_ROUTE_CLASS_HEADER)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("local");
        let execution_path = response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("local_route");
        let request_id = response
            .headers()
            .get(CONTROL_REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("-");
        let request_id = short_request_id(request_id);
        let status_code = response.status().as_u16();
        let elapsed_ms = started_at.elapsed().as_millis() as u64;
        if response.status().is_server_error() {
            warn!(
                event_name = "http_request_failed",
                log_type = "access",
                status = "failed",
                status_code,
                trace_id = %trace_id,
                request_id,
                method = %method,
                path = %path,
                route_class,
                execution_path,
                elapsed_ms,
                "gateway request failed"
            );
        } else if should_downgrade_access_log(&method, &path) {
            trace!(
                event_name = "http_request_completed",
                log_type = "access",
                status = "completed",
                status_code,
                trace_id = %trace_id,
                request_id,
                method = %method,
                path = %path,
                route_class,
                execution_path,
                elapsed_ms,
                "gateway completed request"
            );
        } else {
            info!(
                event_name = "http_request_completed",
                log_type = "access",
                status = "completed",
                status_code,
                trace_id = %trace_id,
                request_id,
                method = %method,
                path = %path,
                route_class,
                execution_path,
                elapsed_ms,
                "gateway completed request"
            );
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::{access_log_middleware, should_downgrade_access_log};
    use crate::constants::{
        CONTROL_REQUEST_ID_HEADER, CONTROL_ROUTE_CLASS_HEADER, EXECUTION_PATH_HEADER,
        TRACE_ID_HEADER,
    };
    use axum::body::Body;
    use axum::http::{Method, Request, Response, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use bytes::Bytes;
    use futures_util::stream;
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;
    use tracing_subscriber::filter::LevelFilter;
    use tracing_subscriber::prelude::*;

    #[derive(Clone, Default)]
    struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

    struct SharedBufferWriter(Arc<Mutex<Vec<u8>>>);

    impl SharedBuffer {
        fn lines(&self) -> Vec<serde_json::Value> {
            String::from_utf8(self.0.lock().expect("buffer should lock").clone())
                .expect("buffer should contain valid utf-8")
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| serde_json::from_str(line).expect("json log line should parse"))
                .collect()
        }
    }

    impl std::io::Write for SharedBufferWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("buffer should lock")
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::writer::MakeWriter<'a> for SharedBuffer {
        type Writer = SharedBufferWriter;

        fn make_writer(&'a self) -> Self::Writer {
            SharedBufferWriter(Arc::clone(&self.0))
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn access_log_emits_completed_events_by_default() {
        let writer = SharedBuffer::default();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .json()
                .flatten_event(true)
                .with_current_span(false)
                .with_span_list(false)
                .with_writer(writer.clone())
                .with_filter(LevelFilter::INFO),
        );
        let dispatch = tracing::Dispatch::new(subscriber);
        let _guard = tracing::dispatcher::set_default(&dispatch);

        let app = Router::new()
            .route(
                "/ok",
                get(|| async {
                    let mut response = Response::new(Body::empty());
                    response.headers_mut().insert(
                        CONTROL_ROUTE_CLASS_HEADER,
                        "local".parse().expect("header should parse"),
                    );
                    response.headers_mut().insert(
                        EXECUTION_PATH_HEADER,
                        "local_route".parse().expect("header should parse"),
                    );
                    response.headers_mut().insert(
                        CONTROL_REQUEST_ID_HEADER,
                        "req-123".parse().expect("header should parse"),
                    );
                    response
                }),
            )
            .layer(axum::middleware::from_fn(access_log_middleware));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/ok")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("request should succeed");

        assert!(response.headers().contains_key(TRACE_ID_HEADER));

        let logs = writer.lines();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0]["event_name"], "http_request_completed");
        assert_eq!(logs[0]["status"], "completed");
        assert_eq!(logs[0]["status_code"], 200);
        assert_eq!(logs[0]["request_id"], "req-123");
        assert_eq!(logs[0]["route_class"], "local");
        assert_eq!(logs[0]["execution_path"], "local_route");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn access_log_propagates_generated_trace_id_to_downstream_handler() {
        let app = Router::new()
            .route(
                "/trace",
                get(|headers: http::HeaderMap| async move {
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(
                            "x-seen-trace-id",
                            headers
                                .get(TRACE_ID_HEADER)
                                .and_then(|value| value.to_str().ok())
                                .unwrap_or("-"),
                        )
                        .body(Body::empty())
                        .expect("response should build")
                }),
            )
            .layer(axum::middleware::from_fn(access_log_middleware));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/trace")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("request should succeed");

        let response_trace_id = response
            .headers()
            .get(TRACE_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .expect("response trace id should exist")
            .to_string();
        let seen_trace_id = response
            .headers()
            .get("x-seen-trace-id")
            .and_then(|value| value.to_str().ok())
            .expect("downstream seen trace id should exist")
            .to_string();

        assert_eq!(seen_trace_id, response_trace_id);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn access_log_shortens_long_request_ids() {
        let writer = SharedBuffer::default();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .json()
                .flatten_event(true)
                .with_current_span(false)
                .with_span_list(false)
                .with_writer(writer.clone())
                .with_filter(LevelFilter::INFO),
        );
        let dispatch = tracing::Dispatch::new(subscriber);
        let _guard = tracing::dispatcher::set_default(&dispatch);

        let app = Router::new()
            .route(
                "/ok",
                get(|| async {
                    let mut response = Response::new(Body::empty());
                    response.headers_mut().insert(
                        CONTROL_ROUTE_CLASS_HEADER,
                        "local".parse().expect("header should parse"),
                    );
                    response.headers_mut().insert(
                        EXECUTION_PATH_HEADER,
                        "local_route".parse().expect("header should parse"),
                    );
                    response.headers_mut().insert(
                        CONTROL_REQUEST_ID_HEADER,
                        "d07e1e94-41b8-409f-a18a-27993ae7ecb1"
                            .parse()
                            .expect("header should parse"),
                    );
                    response
                }),
            )
            .layer(axum::middleware::from_fn(access_log_middleware));

        let _response = app
            .oneshot(
                Request::builder()
                    .uri("/ok")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("request should succeed");

        let logs = writer.lines();
        assert_eq!(logs[0]["request_id"], "d07e1e94");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn access_log_emits_failed_events_by_default_for_server_errors() {
        let writer = SharedBuffer::default();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .json()
                .flatten_event(true)
                .with_current_span(false)
                .with_span_list(false)
                .with_writer(writer.clone())
                .with_filter(LevelFilter::INFO),
        );
        let dispatch = tracing::Dispatch::new(subscriber);
        let _guard = tracing::dispatcher::set_default(&dispatch);

        let app = Router::new()
            .route(
                "/fail",
                get(|| async {
                    Response::builder()
                        .status(StatusCode::BAD_GATEWAY)
                        .header(CONTROL_ROUTE_CLASS_HEADER, "passthrough")
                        .header(EXECUTION_PATH_HEADER, "execution_runtime_sync")
                        .body(Body::empty())
                        .expect("response should build")
                }),
            )
            .layer(axum::middleware::from_fn(access_log_middleware));

        let _response = app
            .oneshot(
                Request::builder()
                    .uri("/fail")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("request should succeed");

        let logs = writer.lines();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0]["event_name"], "http_request_failed");
        assert_eq!(logs[0]["status"], "failed");
        assert_eq!(logs[0]["status_code"], 502);
        assert_eq!(logs[0]["route_class"], "passthrough");
        assert_eq!(logs[0]["execution_path"], "execution_runtime_sync");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn access_log_treats_client_errors_as_completed_events() {
        let writer = SharedBuffer::default();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .json()
                .flatten_event(true)
                .with_current_span(false)
                .with_span_list(false)
                .with_writer(writer.clone())
                .with_filter(LevelFilter::INFO),
        );
        let dispatch = tracing::Dispatch::new(subscriber);
        let _guard = tracing::dispatcher::set_default(&dispatch);

        let app = Router::new()
            .route(
                "/missing",
                get(|| async {
                    Response::builder()
                        .status(StatusCode::UNAUTHORIZED)
                        .header(CONTROL_ROUTE_CLASS_HEADER, "auth")
                        .header(EXECUTION_PATH_HEADER, "local_auth_denied")
                        .body(Body::empty())
                        .expect("response should build")
                }),
            )
            .layer(axum::middleware::from_fn(access_log_middleware));

        let _response = app
            .oneshot(
                Request::builder()
                    .uri("/missing")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("request should succeed");

        let logs = writer.lines();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0]["event_name"], "http_request_completed");
        assert_eq!(logs[0]["status"], "completed");
        assert_eq!(logs[0]["status_code"], 401);
        assert_eq!(logs[0]["route_class"], "auth");
        assert_eq!(logs[0]["execution_path"], "local_auth_denied");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn access_log_emits_completed_events_for_streaming_responses() {
        let writer = SharedBuffer::default();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .json()
                .flatten_event(true)
                .with_current_span(false)
                .with_span_list(false)
                .with_writer(writer.clone())
                .with_filter(LevelFilter::INFO),
        );
        let dispatch = tracing::Dispatch::new(subscriber);
        let _guard = tracing::dispatcher::set_default(&dispatch);

        let app = Router::new()
            .route(
                "/stream",
                get(|| async {
                    let body = Body::from_stream(stream::iter(vec![
                        Ok::<Bytes, std::convert::Infallible>(Bytes::from("chunk-1")),
                        Ok::<Bytes, std::convert::Infallible>(Bytes::from("chunk-2")),
                    ]));
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(CONTROL_ROUTE_CLASS_HEADER, "ai_public")
                        .header(EXECUTION_PATH_HEADER, "execution_runtime_stream")
                        .header(CONTROL_REQUEST_ID_HEADER, "req-stream")
                        .body(body)
                        .expect("response should build")
                }),
            )
            .layer(axum::middleware::from_fn(access_log_middleware));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/stream")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("request should succeed");

        assert_eq!(response.status(), StatusCode::OK);

        let logs = writer.lines();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0]["event_name"], "http_request_completed");
        assert_eq!(logs[0]["status_code"], 200);
        assert_eq!(logs[0]["request_id"], "req-stream");
        assert_eq!(logs[0]["route_class"], "ai_public");
        assert_eq!(logs[0]["execution_path"], "execution_runtime_stream");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn access_log_downgrades_usage_active_polling_to_trace() {
        let writer = SharedBuffer::default();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .json()
                .flatten_event(true)
                .with_current_span(false)
                .with_span_list(false)
                .with_writer(writer.clone())
                .with_filter(LevelFilter::TRACE),
        );
        let dispatch = tracing::Dispatch::new(subscriber);
        let _guard = tracing::dispatcher::set_default(&dispatch);

        let app = Router::new()
            .route(
                "/api/admin/usage/active",
                get(|| async {
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(CONTROL_ROUTE_CLASS_HEADER, "admin_proxy")
                        .header(EXECUTION_PATH_HEADER, "public_proxy_passthrough")
                        .body(Body::empty())
                        .expect("response should build")
                }),
            )
            .layer(axum::middleware::from_fn(access_log_middleware));

        let _response = app
            .oneshot(
                Request::builder()
                    .uri("/api/admin/usage/active?ids=req-1")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("request should succeed");

        let logs = writer.lines();
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0]["level"], "TRACE");
        assert_eq!(logs[0]["event_name"], "http_request_started");
        assert_eq!(logs[1]["level"], "TRACE");
        assert_eq!(logs[1]["event_name"], "http_request_completed");
    }

    #[test]
    fn access_log_marks_usage_active_paths_as_high_frequency() {
        assert!(should_downgrade_access_log(
            &Method::GET,
            "/api/admin/usage/active"
        ));
        assert!(should_downgrade_access_log(
            &Method::GET,
            "/api/admin/usage/active?ids=req-1"
        ));
        assert!(should_downgrade_access_log(
            &Method::GET,
            "/api/users/me/usage/active"
        ));
        assert!(should_downgrade_access_log(
            &Method::GET,
            "/api/admin/usage/records?limit=20"
        ));
        assert!(should_downgrade_access_log(
            &Method::GET,
            "/api/admin/usage/123e4567-e89b-12d3-a456-426614174000?include_bodies=false"
        ));
        assert!(should_downgrade_access_log(
            &Method::GET,
            "/api/admin/monitoring/trace/req-123?attempted_only=false"
        ));
        assert!(should_downgrade_access_log(
            &Method::GET,
            "/api/admin/monitoring/cache/stats"
        ));
        assert!(!should_downgrade_access_log(
            &Method::DELETE,
            "/api/admin/monitoring/cache/affinity/provider/key/model/openai:cli"
        ));
        assert!(!should_downgrade_access_log(&Method::GET, "/v1/responses"));
    }
}
