use aether_contracts::USAGE_SERVER_NOW_UNIX_MS_HEADER;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{self, HeaderValue, Response};
use axum::middleware::Next;

use crate::headers::header_value_str;
use crate::state::{AppState, FrontdoorCorsConfig};

const FRONTDOOR_CREDENTIALS_EXPOSE_HEADERS: &str = "*, x-aether-server-now-unix-ms";

fn append_vary(headers: &mut http::HeaderMap, value: &'static str) {
    headers.append(http::header::VARY, HeaderValue::from_static(value));
}

fn apply_frontdoor_cors_headers(
    headers: &mut http::HeaderMap,
    cors: &FrontdoorCorsConfig,
    origin: &str,
    requested_headers: Option<&str>,
) {
    if cors.allow_any_origin() {
        headers.insert(
            http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static("*"),
        );
    } else if let Ok(value) = HeaderValue::from_str(origin) {
        headers.insert(http::header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
        append_vary(headers, "Origin");
    }
    headers.insert(
        http::header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, PUT, DELETE, PATCH, OPTIONS"),
    );
    headers.insert(
        http::header::ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static(if cors.allow_credentials() {
            FRONTDOOR_CREDENTIALS_EXPOSE_HEADERS
        } else {
            "*"
        }),
    );
    if let Some(value) = requested_headers {
        if let Ok(value) = HeaderValue::from_str(value) {
            headers.insert(http::header::ACCESS_CONTROL_ALLOW_HEADERS, value);
        }
        append_vary(headers, "Access-Control-Request-Headers");
    } else {
        headers.insert(
            http::header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("*"),
        );
    }
    if cors.allow_credentials() {
        headers.insert(
            http::header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
            HeaderValue::from_static("true"),
        );
    }
}

pub(crate) async fn frontdoor_cors_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response<Body> {
    let Some(cors) = state.frontdoor_cors() else {
        return next.run(request).await;
    };

    let origin = header_value_str(request.headers(), http::header::ORIGIN.as_str());
    let requested_headers = header_value_str(
        request.headers(),
        http::header::ACCESS_CONTROL_REQUEST_HEADERS.as_str(),
    );
    let is_preflight = request.method() == http::Method::OPTIONS
        && request
            .headers()
            .contains_key(http::header::ACCESS_CONTROL_REQUEST_METHOD);

    let Some(origin) = origin else {
        return next.run(request).await;
    };

    if !cors.allows_origin(&origin) {
        if is_preflight {
            return Response::builder()
                .status(http::StatusCode::FORBIDDEN)
                .body(Body::empty())
                .expect("cors preflight response should build");
        }
        return next.run(request).await;
    }

    if is_preflight {
        let mut response = Response::builder()
            .status(http::StatusCode::NO_CONTENT)
            .body(Body::empty())
            .expect("cors preflight response should build");
        apply_frontdoor_cors_headers(
            response.headers_mut(),
            &cors,
            &origin,
            requested_headers.as_deref(),
        );
        append_vary(response.headers_mut(), "Access-Control-Request-Method");
        return response;
    }

    let mut response = next.run(request).await;
    apply_frontdoor_cors_headers(
        response.headers_mut(),
        &cors,
        &origin,
        requested_headers.as_deref(),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_exposes_header(value: &HeaderValue, expected: &str) {
        let exposed_headers = value
            .to_str()
            .expect("expose headers should be valid ASCII");
        assert!(
            exposed_headers
                .split(',')
                .map(str::trim)
                .any(|header| header.eq_ignore_ascii_case(expected)),
            "{exposed_headers} should include {expected}"
        );
    }

    #[test]
    fn frontdoor_cors_explicitly_exposes_usage_server_time_for_credentials() {
        let cors = FrontdoorCorsConfig::new(vec!["http://localhost:5173".to_string()], true)
            .expect("cors config should build");
        let mut headers = http::HeaderMap::new();

        apply_frontdoor_cors_headers(&mut headers, &cors, "http://localhost:5173", None);

        let expose_headers = headers
            .get(http::header::ACCESS_CONTROL_EXPOSE_HEADERS)
            .expect("expose headers should be set");
        assert_exposes_header(expose_headers, "*");
        assert_exposes_header(expose_headers, USAGE_SERVER_NOW_UNIX_MS_HEADER);
    }

    #[test]
    fn frontdoor_cors_keeps_wildcard_expose_headers_without_credentials() {
        let cors = FrontdoorCorsConfig::new(vec!["http://localhost:5173".to_string()], false)
            .expect("cors config should build");
        let mut headers = http::HeaderMap::new();

        apply_frontdoor_cors_headers(&mut headers, &cors, "http://localhost:5173", None);

        assert_eq!(
            headers
                .get(http::header::ACCESS_CONTROL_EXPOSE_HEADERS)
                .expect("expose headers should be set"),
            "*"
        );
    }
}
