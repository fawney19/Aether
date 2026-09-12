use super::{sample_auth_user, sample_auth_wallet, start_auth_gateway_with_state};
use axum::http::{header, StatusCode};
use chrono::Utc;
use serde_json::json;

fn refresh_cookie(response: &reqwest::Response, secure: bool) -> String {
    let cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(cookie.starts_with("aether_refresh_token="));
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("Path=/api/auth"));
    assert_eq!(
        cookie
            .split(';')
            .any(|attribute| attribute.trim() == "Secure"),
        secure
    );
    if !secure {
        assert!(!cookie.contains("SameSite=None"));
        assert!(cookie.contains("SameSite=Lax"));
    }
    assert_eq!(
        response.headers().get(header::CACHE_CONTROL).unwrap(),
        "no-store"
    );
    cookie.to_string()
}

#[tokio::test]
async fn gateway_auth_refresh_cookie_roundtrip_adapts_to_http_and_https() {
    for (origin_scheme, forwarded_proto, secure) in [
        ("http", None, false),
        ("https", None, true),
        ("https", Some("https"), true),
        ("http", Some("https"), true),
    ] {
        let now = Utc::now();
        let (gateway_url, upstream_hits, gateway_handle, upstream_handle) =
            start_auth_gateway_with_state(
                sample_auth_user(now),
                sample_auth_wallet("user-auth-1", now),
                [],
            )
            .await;
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            gateway_url
                .replacen("http:", &format!("{origin_scheme}:"), 1)
                .parse()
                .unwrap(),
        );
        headers.insert(
            "x-client-device-id",
            "cookie-roundtrip-device".parse().unwrap(),
        );
        if let Some(proto) = forwarded_proto {
            headers.insert("x-forwarded-proto", proto.parse().unwrap());
        }
        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .unwrap();
        let login = client.post(format!("{gateway_url}/api/auth/login"))
            .json(&json!({ "email": "alice@example.com", "password": "secret123", "auth_type": "local" }))
            .send().await.unwrap();
        assert_eq!(login.status(), StatusCode::OK);
        let mut cookie = refresh_cookie(&login, secure);

        for _ in 0..3 {
            let refreshed = client
                .post(format!("{gateway_url}/api/auth/refresh"))
                .header(header::COOKIE, cookie.split(';').next().unwrap())
                .send()
                .await
                .unwrap();
            assert_eq!(refreshed.status(), StatusCode::OK);
            let rotated = refresh_cookie(&refreshed, secure);
            assert_ne!(rotated, cookie);
            cookie = rotated;
            let payload: serde_json::Value = refreshed.json().await.unwrap();
            let current_user = client
                .get(format!("{gateway_url}/api/auth/me"))
                .bearer_auth(payload["access_token"].as_str().unwrap())
                .send()
                .await
                .unwrap();
            assert_eq!(current_user.status(), StatusCode::OK);
        }

        let logout = client
            .post(format!("{gateway_url}/api/auth/logout"))
            .header(header::COOKIE, cookie.split(';').next().unwrap())
            .send()
            .await
            .unwrap();
        assert_eq!(logout.status(), StatusCode::OK);
        assert!(refresh_cookie(&logout, secure).contains("Max-Age=0"));

        let revoked = client
            .post(format!("{gateway_url}/api/auth/refresh"))
            .header(header::COOKIE, cookie.split(';').next().unwrap())
            .send()
            .await
            .unwrap();
        assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);
        assert!(refresh_cookie(&revoked, secure).contains("Max-Age=0"));

        let missing = client
            .post(format!("{gateway_url}/api/auth/refresh"))
            .send()
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);
        assert!(refresh_cookie(&missing, secure).contains("Max-Age=0"));
        assert_eq!(*upstream_hits.lock().unwrap(), 0);
        gateway_handle.abort();
        upstream_handle.abort();
    }
}
