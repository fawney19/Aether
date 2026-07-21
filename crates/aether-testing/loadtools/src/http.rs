use aether_http::{build_http_client, HttpClientConfig};

pub fn json_body(value: serde_json::Value) -> serde_json::Value {
    value
}

pub fn test_http_client_config() -> HttpClientConfig {
    HttpClientConfig {
        connect_timeout_ms: Some(1_000),
        request_timeout_ms: Some(5_000),
        user_agent: Some("aether-testkit".to_string()),
        ..HttpClientConfig::default()
    }
}

pub fn test_http_client() -> reqwest::Client {
    build_http_client(&test_http_client_config()).expect("failed to build test HTTP client")
}
