use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{self, HeaderValue, Response};
use axum::middleware::Next;

use crate::headers::header_value_str;
use crate::state::{AppState, FrontdoorCorsConfig};

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
        HeaderValue::from_static(if headers.contains_key("x-aether-body-field") {
            "*, X-Aether-Body-Encoding, X-Aether-Body-Field, X-Aether-Body-Error, X-Aether-Usage-Id"
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

    #[test]
    fn raw_body_headers_are_explicitly_exposed_for_credentialed_requests() {
        let cors =
            FrontdoorCorsConfig::new(vec!["https://console.example".to_string()], true).unwrap();
        let mut headers = http::HeaderMap::new();
        headers.insert(
            "x-aether-body-field",
            HeaderValue::from_static("request_body"),
        );
        apply_frontdoor_cors_headers(&mut headers, &cors, "https://console.example", None);
        let exposed = headers[http::header::ACCESS_CONTROL_EXPOSE_HEADERS]
            .to_str()
            .unwrap()
            .to_ascii_lowercase();
        for name in [
            "x-aether-body-encoding",
            "x-aether-body-field",
            "x-aether-body-error",
            "x-aether-usage-id",
        ] {
            assert!(exposed.split(',').any(|header| header.trim() == name));
        }
        assert_eq!(
            headers[http::header::ACCESS_CONTROL_ALLOW_CREDENTIALS],
            "true"
        );
    }
}
