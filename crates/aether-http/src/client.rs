use std::time::Duration;

use reqwest::header::HeaderMap;

use crate::HttpClientConfig;

pub fn apply_http_client_config(
    mut builder: reqwest::ClientBuilder,
    config: &HttpClientConfig,
) -> reqwest::ClientBuilder {
    if config.use_rustls_tls {
        builder = builder.use_rustls_tls();
    }
    if let Some(timeout_ms) = config.connect_timeout_ms {
        builder = builder.connect_timeout(Duration::from_millis(timeout_ms));
    }
    if let Some(timeout_ms) = config.request_timeout_ms {
        builder = builder.timeout(Duration::from_millis(timeout_ms));
    }
    if let Some(timeout_ms) = config.pool_idle_timeout_ms {
        builder = builder.pool_idle_timeout(Duration::from_millis(timeout_ms));
    }
    if let Some(max_idle) = config.pool_max_idle_per_host {
        builder = builder.pool_max_idle_per_host(max_idle);
    }

    builder = builder.tcp_keepalive(config.tcp_keepalive_ms.map(Duration::from_millis));
    builder = builder.tcp_nodelay(config.tcp_nodelay);

    if config.http2_adaptive_window {
        builder = builder.http2_adaptive_window(true);
    }
    if let Some(user_agent) = &config.user_agent {
        builder = builder.user_agent(user_agent.clone());
    }
    builder
}

pub fn build_http_client(config: &HttpClientConfig) -> Result<reqwest::Client, reqwest::Error> {
    build_http_client_with_headers(config, HeaderMap::new())
}

pub fn build_http_client_with_headers(
    config: &HttpClientConfig,
    default_headers: HeaderMap,
) -> Result<reqwest::Client, reqwest::Error> {
    // Shared clients must not silently inherit HTTP(S)_PROXY. Callers that
    // need a proxy install the explicitly configured URL below.
    let mut builder = apply_http_client_config(reqwest::Client::builder().no_proxy(), config);
    if let Some(proxy_url) = config
        .proxy_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        builder = builder.proxy(reqwest::Proxy::all(proxy_url)?);
    }
    if !default_headers.is_empty() {
        builder = builder.default_headers(default_headers);
    }
    builder.build()
}

#[cfg(test)]
mod tests {
    use reqwest::header::{HeaderMap, HeaderValue};

    use super::build_http_client_with_headers;
    use crate::HttpClientConfig;

    #[test]
    fn builds_client_with_default_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-test", HeaderValue::from_static("ok"));
        let config = HttpClientConfig {
            connect_timeout_ms: Some(100),
            request_timeout_ms: Some(500),
            ..HttpClientConfig::default()
        };

        let client = build_http_client_with_headers(&config, headers);
        assert!(client.is_ok());
    }
}
