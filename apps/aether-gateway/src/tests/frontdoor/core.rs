use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::tests::{
    any, attach_static_frontend, build_router, start_server, Arc, Body, Mutex, Request, Router,
    StatusCode, READYZ_PATH,
};

#[tokio::test]
async fn gateway_exposes_readyz_without_proxying_upstream() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("proxied"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}{READYZ_PATH}"))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["status"], "ready");
    assert_eq!(payload["component"], "aether-gateway");
    assert_eq!(payload["warmup_status"], "disabled");
    assert_eq!(payload["gate_readiness"], false);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_public_health_without_proxying_upstream() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("proxied"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/health"))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["status"], "healthy");
    assert_eq!(payload["database_pool"]["source"], "rust_frontdoor");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_public_service_health_without_proxying_upstream() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("proxied"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/v1/health"))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["status"], "ok");
    assert!(payload["stats"].is_object());
    assert!(payload["dependencies"]["database"]["status"].is_string());
    assert!(payload["dependencies"]["redis"]["status"].is_string());
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_serves_frontend_routes_and_assets_without_shadowing_public_api() {
    let unique_suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be monotonic enough for tests")
        .as_nanos();
    let static_dir =
        std::env::temp_dir().join(format!("aether-gateway-static-test-{unique_suffix}"));
    let assets_dir = static_dir.join("assets");
    fs::create_dir_all(&assets_dir).expect("static assets dir should be created");
    fs::write(
        static_dir.join("index.html"),
        "<!doctype html><html><body>Aether Frontend</body></html>",
    )
    .expect("index.html should be written");
    fs::write(assets_dir.join("app.js"), "console.log('frontend asset');")
        .expect("asset file should be written");

    let gateway =
        attach_static_frontend(build_router().expect("gateway should build"), &static_dir);
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    let response = client
        .get(format!("{gateway_url}/"))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let body = response.text().await.expect("html body should be readable");
    assert!(content_type.starts_with("text/html"));
    assert!(body.contains("Aether Frontend"));

    let response = client
        .get(format!("{gateway_url}/guide"))
        .send()
        .await
        .expect("spa request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.expect("spa body should be readable");
    assert!(body.contains("Aether Frontend"));

    let response = client
        .get(format!("{gateway_url}/assets/app.js"))
        .send()
        .await
        .expect("asset request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .text()
            .await
            .expect("asset body should be readable"),
        "console.log('frontend asset');"
    );

    let response = client
        .get(format!("{gateway_url}/api/public/site-info"))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["site_name"], "Aether");
    assert_eq!(payload["site_subtitle"], "AI Gateway");

    gateway_handle.abort();
    let _ = fs::remove_dir_all(&static_dir);
}
