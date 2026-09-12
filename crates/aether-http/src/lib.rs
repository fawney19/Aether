mod client;
mod config;
mod dns;
mod header_security;
mod response_body;
mod retry;

pub use client::{apply_http_client_config, build_http_client, build_http_client_with_headers};
pub use config::{HttpClientConfig, HttpRetryConfig};
pub use dns::{
    lookup_host_with_limits, parse_ip_literal_host, DEFAULT_DNS_LOOKUP_TIMEOUT,
    MAX_DNS_RESOLVED_ADDRESSES,
};
pub use header_security::{
    connection_declared_header_names, is_https_or_loopback_http_url, is_ipv4_benchmarking_fake_ip,
    is_private_or_reserved_ip, url_has_literal_loopback_host,
};
pub use response_body::{read_response_bytes_with_limit, ResponseBodyReadError};
pub use retry::jittered_delay_for_retry;
