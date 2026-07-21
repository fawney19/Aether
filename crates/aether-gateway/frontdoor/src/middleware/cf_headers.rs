use axum::{extract::Request, middleware::Next, response::Response, Router};
use http::{header::HeaderName, HeaderMap};

const CF_EXACT_HEADERS: &[&str] = &["cdn-loop", "true-client-ip"];

#[derive(Clone, Debug)]
pub struct CfConnectingIp(pub String);

fn should_strip_cf_header(name: &HeaderName) -> bool {
    let normalized = name.as_str();
    normalized.starts_with("cf-") || CF_EXACT_HEADERS.contains(&normalized)
}

fn cf_connecting_ip(headers: &HeaderMap) -> Option<String> {
    headers
        .get("cf-connecting-ip")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(45).collect())
}

fn strip_cf_headers(headers: &mut HeaderMap) {
    let to_remove: Vec<_> = headers
        .keys()
        .filter(|name| should_strip_cf_header(name))
        .cloned()
        .collect();
    for name in to_remove {
        headers.remove(name);
    }
}

pub fn apply_cf_header_stripping(router: Router) -> Router {
    router.layer(axum::middleware::from_fn(strip_cf_headers_middleware))
}

pub async fn strip_cf_headers_middleware(mut request: Request, next: Next) -> Response {
    if let Some(client_ip) = cf_connecting_ip(request.headers()) {
        request.extensions_mut().insert(CfConnectingIp(client_ip));
    }
    strip_cf_headers(request.headers_mut());

    let mut response = next.run(request).await;
    strip_cf_headers(response.headers_mut());
    response
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::routing::any;
    use axum::Router;
    use http::{HeaderValue, Request, Response};
    use tower::ServiceExt;

    use super::apply_cf_header_stripping;

    #[tokio::test]
    async fn strips_cf_prefixed_and_exact_headers_from_request_and_response() {
        let app = apply_cf_header_stripping(Router::new().route(
            "/",
            any(|headers: http::HeaderMap| async move {
                let leaked = headers.contains_key("cf-ipcity")
                    || headers.contains_key("cf-ray")
                    || headers.contains_key("cf-connecting-ip")
                    || headers.contains_key("true-client-ip")
                    || headers.contains_key("cdn-loop");
                let mut response =
                    Response::new(Body::from(if leaked { "leaked" } else { "clean" }));
                response.headers_mut().insert(
                    http::header::HeaderName::from_static("cf-ipcity"),
                    HeaderValue::from_static("Shanghai"),
                );
                response.headers_mut().insert(
                    http::header::HeaderName::from_static("cf-cache-status"),
                    HeaderValue::from_static("HIT"),
                );
                response.headers_mut().insert(
                    http::header::HeaderName::from_static("true-client-ip"),
                    HeaderValue::from_static("1.1.1.1"),
                );
                response.headers_mut().insert(
                    http::header::HeaderName::from_static("cdn-loop"),
                    HeaderValue::from_static("cloudflare"),
                );
                response
            }),
        ));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("cf-ipcity", "Shanghai")
                    .header("cf-ray", "abc123")
                    .header("cf-connecting-ip", "203.0.113.10")
                    .header("true-client-ip", "1.1.1.1")
                    .header("cdn-loop", "cloudflare")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("request should succeed");

        assert!(response.headers().get("cf-ipcity").is_none());
        assert!(response.headers().get("cf-cache-status").is_none());
        assert!(response.headers().get("cf-connecting-ip").is_none());
        assert!(response.headers().get("true-client-ip").is_none());
        assert!(response.headers().get("cdn-loop").is_none());

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should be readable");
        assert_eq!(body.as_ref(), b"clean");
    }

    #[tokio::test]
    async fn preserves_non_cf_headers() {
        let app = apply_cf_header_stripping(Router::new().route(
            "/",
            any(|headers: http::HeaderMap| async move {
                let mut response = Response::new(Body::from(
                    headers
                        .get("x-custom-header")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                ));
                response.headers_mut().insert(
                    http::header::HeaderName::from_static("x-custom-response"),
                    HeaderValue::from_static("kept"),
                );
                response
            }),
        ));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("x-custom-header", "kept")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("request should succeed");

        assert_eq!(
            response
                .headers()
                .get("x-custom-response")
                .and_then(|value| value.to_str().ok()),
            Some("kept")
        );
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should be readable");
        assert_eq!(body.as_ref(), b"kept");
    }
}
