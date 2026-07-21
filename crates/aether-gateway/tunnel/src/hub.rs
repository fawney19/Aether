use base64::Engine as _;
use http::HeaderMap;

pub const MAX_TUNNEL_STREAMS: usize = 2_048;

pub fn resolve_proxy_max_streams(headers: &HeaderMap, fallback: usize) -> usize {
    headers
        .get("x-tunnel-max-streams")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(fallback)
        .clamp(1, MAX_TUNNEL_STREAMS)
}

pub fn resolve_proxy_node_name(headers: &HeaderMap, node_id: &str) -> String {
    if let Some(decoded) = headers
        .get(aether_contracts::tunnel::TUNNEL_NODE_NAME_B64_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(value.trim())
                .ok()
        })
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && value.chars().count() <= 100)
    {
        return decoded;
    }

    headers
        .get("x-node-name")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(node_id)
        .to_string()
}

pub fn resolve_proxy_protocol_version(headers: &HeaderMap) -> u8 {
    headers
        .get(aether_contracts::tunnel::TUNNEL_PROTOCOL_VERSION_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u8>().ok())
        .filter(|value| *value >= 1)
        .map(|value| value.min(aether_contracts::tunnel::CURRENT_TUNNEL_PROTOCOL_VERSION))
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::{
        resolve_proxy_max_streams, resolve_proxy_node_name, resolve_proxy_protocol_version,
    };
    use base64::Engine as _;
    use http::{HeaderMap, HeaderValue};

    #[test]
    fn resolves_bounded_proxy_capacity() {
        let mut headers = HeaderMap::new();
        headers.insert("x-tunnel-max-streams", HeaderValue::from_static("8"));
        assert_eq!(resolve_proxy_max_streams(&headers, 128), 8);

        headers.insert("x-tunnel-max-streams", HeaderValue::from_static("9999"));
        assert_eq!(resolve_proxy_max_streams(&headers, 128), 2_048);
    }

    #[test]
    fn resolves_protocol_version_with_v1_fallback() {
        let mut headers = HeaderMap::new();
        assert_eq!(resolve_proxy_protocol_version(&headers), 1);

        headers.insert(
            aether_contracts::tunnel::TUNNEL_PROTOCOL_VERSION_HEADER,
            HeaderValue::from_static("2"),
        );
        assert_eq!(resolve_proxy_protocol_version(&headers), 2);
    }

    #[test]
    fn resolves_utf8_node_name_with_legacy_fallback() {
        let mut headers = HeaderMap::new();
        let utf8_name = "\u{65e5}\u{672c}\u{8282}\u{70b9}";
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(utf8_name);
        headers.insert(
            aether_contracts::tunnel::TUNNEL_NODE_NAME_B64_HEADER,
            HeaderValue::from_str(&encoded).expect("encoded header value should parse"),
        );
        assert_eq!(resolve_proxy_node_name(&headers, "node-1"), utf8_name);

        headers.remove(aether_contracts::tunnel::TUNNEL_NODE_NAME_B64_HEADER);
        headers.insert("x-node-name", HeaderValue::from_static("edge-1"));
        assert_eq!(resolve_proxy_node_name(&headers, "node-1"), "edge-1");
    }
}
