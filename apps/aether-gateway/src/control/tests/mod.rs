use http::Uri;

use super::{classify_control_route, GatewayPublicRequestContext};

fn headers(items: &[(&str, &str)]) -> http::HeaderMap {
    let mut headers = http::HeaderMap::new();
    for (name, value) in items {
        headers.insert(
            http::header::HeaderName::from_bytes(name.as_bytes()).expect("valid header name"),
            http::HeaderValue::from_str(value).expect("valid header value"),
        );
    }
    headers
}

#[test]
fn builds_public_request_context_from_request_parts() {
    let mut headers = headers(&[
        (http::header::HOST.as_str(), "api.example.test"),
        (http::header::CONTENT_TYPE.as_str(), "application/json"),
    ]);
    headers.insert("x-app", http::HeaderValue::from_static("gemini-cli"));
    let uri: Uri = "/v1beta/models/gemini-2.5-pro:generateContent?alt=sse"
        .parse()
        .expect("uri should parse");
    let decision = classify_control_route(&http::Method::POST, &uri, &headers);

    let context = GatewayPublicRequestContext::from_request_parts(
        "trace-123",
        &http::Method::POST,
        &uri,
        &headers,
        decision,
    );

    assert_eq!(context.trace_id, "trace-123");
    assert_eq!(context.request_method, http::Method::POST);
    assert_eq!(
        context.request_path,
        "/v1beta/models/gemini-2.5-pro:generateContent"
    );
    assert_eq!(context.request_query_string.as_deref(), Some("alt=sse"));
    assert_eq!(
        context.request_path_and_query(),
        "/v1beta/models/gemini-2.5-pro:generateContent?alt=sse"
    );
    assert_eq!(
        context.request_content_type.as_deref(),
        Some("application/json")
    );
    assert_eq!(context.host_header.as_deref(), Some("api.example.test"));
    assert_eq!(
        context
            .control_decision
            .as_ref()
            .and_then(|value| value.route_family.as_deref()),
        Some("gemini")
    );
    assert_eq!(
        context
            .control_decision
            .as_ref()
            .and_then(|value| value.route_kind.as_deref()),
        Some("generate_content")
    );
    assert_eq!(
        context
            .control_decision
            .as_ref()
            .and_then(|value| value.request_auth_channel.as_deref()),
        Some("bearer_like")
    );
}

mod admin_adaptive;
mod admin_api_keys;
mod admin_billing;
mod admin_core;
mod admin_endpoints;
mod admin_monitoring;
mod admin_oauth;
mod admin_payments;
mod admin_pool;
mod admin_provider_ops;
mod admin_provider_query;
mod admin_provider_strategy;
mod admin_providers_models;
mod admin_proxy_nodes;
mod admin_security;
mod admin_stats;
mod admin_usage;
mod admin_users;
mod admin_video_tasks;
mod admin_wallets;
mod ai;
mod internal;
mod public_support;
