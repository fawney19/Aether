//! Network utility functions (public IP detection, region detection).
//!
//! These are standalone helpers not tied to any specific client or service.

use std::net::IpAddr;

use aether_http::{
    build_http_client, is_private_or_reserved_ip, read_response_bytes_with_limit, HttpClientConfig,
};
use tracing::{debug, info};
use url::Url;

const MAX_NETWORK_DISCOVERY_RESPONSE_BYTES: usize = 16 * 1024;

/// Auto-detect public IP by querying external services.
pub async fn detect_public_ip() -> anyhow::Result<String> {
    let endpoints = [
        "https://api.ipify.org",
        "https://ifconfig.me/ip",
        "https://icanhazip.com",
    ];

    let client = build_http_client(&HttpClientConfig {
        request_timeout_ms: Some(5_000),
        user_agent: Some("aether-tunnel/net".to_string()),
        ..HttpClientConfig::default()
    })?;

    for endpoint in &endpoints {
        match client.get(*endpoint).send().await {
            Ok(resp) if resp.status().is_success() => {
                match read_response_bytes_with_limit(resp, MAX_NETWORK_DISCOVERY_RESPONSE_BYTES)
                    .await
                {
                    Ok(body) => {
                        let text = String::from_utf8_lossy(&body);
                        if let Some(ip) = parse_ip(text.as_ref()) {
                            let ip = ip.to_string();
                            info!(ip = %ip, source = %endpoint, "detected public IP");
                            return Ok(ip);
                        }
                        debug!(endpoint = %endpoint, "IP detection response was not a valid IP");
                    }
                    Err(error) => {
                        debug!(endpoint = %endpoint, error = %error, "IP detection response rejected");
                    }
                }
            }
            Ok(resp) => {
                debug!(endpoint = %endpoint, status = %resp.status(), "IP detection failed");
            }
            Err(e) => {
                debug!(endpoint = %endpoint, error = %e, "IP detection failed");
            }
        }
    }

    anyhow::bail!("failed to detect public IP from any source; use --public-ip")
}

/// Auto-detect geographic region from a public IP address.
///
/// Uses multiple providers with HTTPS preferred.  Falls back to ip-api.com
/// over plain HTTP (their free tier doesn't support HTTPS).
/// This is best-effort and non-sensitive -- region detection should never
/// block startup.
pub async fn detect_region(ip: &str) -> Option<String> {
    // The value may come directly from the command line/environment.  Parse
    // it before putting it into a URL and avoid disclosing private or reserved
    // addresses to third-party geolocation services.  We intentionally do not
    // reject such values from registration: controlled internal deployments
    // can still advertise their configured address and region explicitly.
    let ip = parse_ip(ip)?;
    if is_private_or_reserved_ip(ip) {
        debug!(ip = %ip, "skipping region detection for private or reserved IP");
        return None;
    }
    let ip = ip.to_string();

    // Try HTTPS provider first
    let https_url = ipinfo_url(&ip)?;

    let client = build_http_client(&HttpClientConfig {
        request_timeout_ms: Some(5_000),
        user_agent: Some("aether-tunnel/net".to_string()),
        ..HttpClientConfig::default()
    })
    .ok()?;

    // Try ipinfo.io (HTTPS, returns plain text country code)
    if let Ok(resp) = client.get(&https_url).send().await {
        if resp.status().is_success() {
            if let Ok(body) =
                read_response_bytes_with_limit(resp, MAX_NETWORK_DISCOVERY_RESPONSE_BYTES).await
            {
                let text = String::from_utf8_lossy(&body);
                if let Some(code) = normalize_country_code(text.as_ref()) {
                    info!(region = %code, ip = %ip, source = "ipinfo.io", "detected region");
                    return Some(code);
                }
            }
        }
    }

    // Fallback: ip-api.com (HTTP only on free tier, non-sensitive data)
    let http_url = ip_api_url(&ip)?;
    match client.get(&http_url).send().await {
        Ok(resp) if resp.status().is_success() => {
            let body = read_response_bytes_with_limit(resp, MAX_NETWORK_DISCOVERY_RESPONSE_BYTES)
                .await
                .ok()?;
            let body: serde_json::Value = serde_json::from_slice(&body).ok()?;
            let code = normalize_country_code(body.get("countryCode")?.as_str()?)?;
            info!(region = %code, ip = %ip, source = "ip-api.com", "detected region");
            Some(code)
        }
        _ => {
            debug!(ip = %ip, "region detection failed");
            None
        }
    }
}

fn parse_ip(value: &str) -> Option<IpAddr> {
    let value = value.trim();
    if value.is_empty() || value.len() > 45 {
        return None;
    }
    value.parse().ok()
}

fn normalize_country_code(value: &str) -> Option<String> {
    let value = value.trim();
    if !(2..=3).contains(&value.len()) || !value.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        return None;
    }
    Some(value.to_ascii_uppercase())
}

fn ipinfo_url(ip: &str) -> Option<String> {
    let mut url = Url::parse("https://ipinfo.io").ok()?;
    url.path_segments_mut().ok()?.push(ip).push("country");
    Some(url.into())
}

fn ip_api_url(ip: &str) -> Option<String> {
    let mut url = Url::parse("http://ip-api.com").ok()?;
    url.path_segments_mut().ok()?.push("json").push(ip);
    url.query_pairs_mut().append_pair("fields", "countryCode");
    Some(url.into())
}

#[cfg(test)]
mod tests {
    use super::{ip_api_url, ipinfo_url, normalize_country_code, parse_ip};

    #[test]
    fn parses_only_bounded_ip_values() {
        assert_eq!(parse_ip(" 8.8.8.8\n"), Some("8.8.8.8".parse().unwrap()));
        assert_eq!(
            parse_ip("2001:4860:4860::8888"),
            Some("2001:4860:4860::8888".parse().unwrap())
        );
        for value in ["", "8.8.8.8?x=1", "8.8.8.8\nX-Injected: yes", "not-an-ip"] {
            assert!(parse_ip(value).is_none(), "accepted invalid IP: {value:?}");
        }
    }

    #[test]
    fn builds_urls_without_allowing_path_or_query_injection() {
        let ip = "2001:4860:4860::8888";
        let info = ipinfo_url(ip).expect("fixed URL should parse");
        let info = url::Url::parse(&info).unwrap();
        assert_eq!(info.host_str(), Some("ipinfo.io"));
        assert_eq!(
            info.path_segments().unwrap().collect::<Vec<_>>(),
            ["2001:4860:4860::8888", "country"]
        );
        assert_eq!(info.query(), None);
        assert_eq!(info.fragment(), None);

        let api = ip_api_url(ip).expect("fixed URL should parse");
        let api = url::Url::parse(&api).unwrap();
        assert_eq!(api.host_str(), Some("ip-api.com"));
        assert_eq!(api.query(), Some("fields=countryCode"));
        assert_eq!(
            api.path_segments().unwrap().collect::<Vec<_>>(),
            ["json", "2001:4860:4860::8888"]
        );
        assert_eq!(api.fragment(), None);
    }

    #[test]
    fn country_codes_are_ascii_and_normalized() {
        assert_eq!(normalize_country_code(" us\n"), Some("US".to_string()));
        assert_eq!(normalize_country_code("GBR"), Some("GBR".to_string()));
        for value in ["", "U", "US!", "US\nX", "中国"] {
            assert!(
                normalize_country_code(value).is_none(),
                "accepted {value:?}"
            );
        }
    }
}
