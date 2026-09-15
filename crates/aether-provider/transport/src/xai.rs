pub mod video;

use std::collections::BTreeMap;

use aether_ai_formats::normalize_api_format_alias;
use serde_json::Value;

use crate::snapshot::GatewayProviderTransportSnapshot;

pub const XAI_PROVIDER_TYPE: &str = "xai";
pub const XAI_CHAT_PROXY_BASE_URL: &str = "https://cli-chat-proxy.grok.com/v1";
pub const XAI_API_BASE_URL: &str = "https://api.x.ai/v1";
pub const XAI_CLIENT_VERSION: &str = "0.2.120";
pub const XAI_TOKEN_AUTH_HEADER: &str = "x-xai-token-auth";
pub const XAI_TOKEN_AUTH_VALUE: &str = "xai-grok-cli";
pub const XAI_CLIENT_VERSION_HEADER: &str = "x-grok-client-version";
pub const XAI_CLIENT_IDENTIFIER_HEADER: &str = "x-grok-client-identifier";
pub const XAI_CLIENT_IDENTIFIER_VALUE: &str = "grok-shell";
pub const XAI_AUTHENTICATE_RESPONSE_HEADER: &str = "x-authenticateresponse";
pub const XAI_AUTHENTICATE_RESPONSE_VALUE: &str = "authenticate-response";

pub fn xai_cli_user_agent() -> String {
    format!("xai-grok-workspace/{XAI_CLIENT_VERSION}")
}

pub fn is_xai_provider_transport(transport: &GatewayProviderTransportSnapshot) -> bool {
    transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case(XAI_PROVIDER_TYPE)
}

pub fn xai_uses_official_api(api_format: &str) -> bool {
    matches!(
        normalize_api_format_alias(api_format).as_str(),
        "openai:responses:compact"
    )
}

pub fn resolved_xai_upstream_base_url(
    transport: &GatewayProviderTransportSnapshot,
    api_format: &str,
) -> Option<String> {
    if !is_xai_provider_transport(transport) {
        return None;
    }
    let stored = transport.endpoint.base_url.trim();
    if xai_uses_official_api(api_format) {
        if stored.is_empty()
            || is_cli_chat_proxy_base_url(stored)
            || is_official_api_base_url(stored)
        {
            return Some(XAI_API_BASE_URL.to_string());
        }
        return Some(trim_base_url(stored));
    }
    if xai_using_api(transport) {
        if stored.is_empty() || is_cli_chat_proxy_base_url(stored) {
            return Some(XAI_API_BASE_URL.to_string());
        }
        return Some(trim_base_url(stored));
    }
    if stored.is_empty() || is_official_api_base_url(stored) {
        return Some(XAI_CHAT_PROXY_BASE_URL.to_string());
    }
    Some(trim_base_url(stored))
}

pub fn resolved_xai_request_base_url(
    transport: &GatewayProviderTransportSnapshot,
    api_format: &str,
) -> String {
    resolved_xai_upstream_base_url(transport, api_format)
        .unwrap_or_else(|| trim_base_url(&transport.endpoint.base_url))
}

pub fn should_attach_cli_identity_headers(
    transport: &GatewayProviderTransportSnapshot,
    api_format: &str,
) -> bool {
    if !is_xai_provider_transport(transport) {
        return false;
    }
    if xai_uses_official_api(api_format) {
        return false;
    }
    resolved_xai_upstream_base_url(transport, api_format)
        .as_deref()
        .is_some_and(is_cli_chat_proxy_base_url)
}

pub fn insert_cli_identity_headers(headers: &mut BTreeMap<String, String>) {
    let user_agent = xai_cli_user_agent();
    for (name, value) in [
        (XAI_TOKEN_AUTH_HEADER, XAI_TOKEN_AUTH_VALUE),
        (XAI_CLIENT_VERSION_HEADER, XAI_CLIENT_VERSION),
        ("user-agent", user_agent.as_str()),
        (XAI_CLIENT_IDENTIFIER_HEADER, XAI_CLIENT_IDENTIFIER_VALUE),
        (
            XAI_AUTHENTICATE_RESPONSE_HEADER,
            XAI_AUTHENTICATE_RESPONSE_VALUE,
        ),
    ] {
        if !headers
            .keys()
            .any(|existing| existing.eq_ignore_ascii_case(name))
        {
            headers.insert(name.to_string(), value.to_string());
        }
    }
}

pub fn insert_cli_identity_headers_if_needed(
    transport: &GatewayProviderTransportSnapshot,
    api_format: &str,
    headers: &mut BTreeMap<String, String>,
) {
    if should_attach_cli_identity_headers(transport, api_format) {
        insert_cli_identity_headers(headers);
    }
}

pub fn xai_auth_uses_api(auth_type: &str, decrypted_auth_config: Option<&str>) -> bool {
    if let Some(value) = auth_config_using_api(decrypted_auth_config) {
        return value;
    }
    let auth_type = auth_type.trim().to_ascii_lowercase();
    if auth_type == "oauth" || auth_config_has_refresh_token(decrypted_auth_config) {
        return false;
    }
    matches!(auth_type.as_str(), "api_key" | "bearer" | "apikey")
}

pub fn extract_xai_user_id_from_auth_config(raw_auth_config: Option<&str>) -> Option<String> {
    let value = parse_auth_config(raw_auth_config)?;
    extract_xai_user_id_from_value(&value)
}

pub fn extract_xai_user_id_from_value(value: &Value) -> Option<String> {
    const PATHS: &[&[&str]] = &[
        &["userId"],
        &["user_id"],
        &["id"],
        &["sub"],
        &["user", "userId"],
        &["user", "id"],
        &["user", "user_id"],
        &["user", "sub"],
    ];
    PATHS.iter().find_map(|path| {
        let mut current = value;
        for key in *path {
            current = current.get(*key)?;
        }
        coerce_xai_id(current)
    })
}

fn xai_using_api(transport: &GatewayProviderTransportSnapshot) -> bool {
    xai_auth_uses_api(
        transport.key.auth_type.as_str(),
        transport.key.decrypted_auth_config.as_deref(),
    )
}

fn coerce_xai_id(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => {
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        Value::Number(number) => {
            let rendered = number.to_string();
            (!rendered.is_empty()).then_some(rendered)
        }
        _ => None,
    }
}

fn auth_config_using_api(raw_auth_config: Option<&str>) -> Option<bool> {
    let value = parse_auth_config(raw_auth_config)?;
    let using_api = value.get("using_api")?;
    match using_api {
        Value::Bool(value) => Some(*value),
        Value::String(value) => value.trim().parse::<bool>().ok(),
        _ => None,
    }
}

fn auth_config_has_refresh_token(raw_auth_config: Option<&str>) -> bool {
    let value = match parse_auth_config(raw_auth_config) {
        Some(value) => value,
        None => return false,
    };
    ["refresh_token", "refreshToken"]
        .iter()
        .find_map(|field| value.get(*field).and_then(Value::as_str))
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
}

fn parse_auth_config(raw_auth_config: Option<&str>) -> Option<Value> {
    raw_auth_config
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| serde_json::from_str::<Value>(value).ok())
}

fn trim_base_url(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
}

fn normalize_base_url(url: &str) -> String {
    trim_base_url(url).to_ascii_lowercase()
}

fn is_official_api_base_url(url: &str) -> bool {
    normalize_base_url(url) == normalize_base_url(XAI_API_BASE_URL)
}

fn is_cli_chat_proxy_base_url(url: &str) -> bool {
    normalize_base_url(url) == normalize_base_url(XAI_CHAT_PROXY_BASE_URL)
}

#[cfg(test)]
mod tests {
    use super::{
        insert_cli_identity_headers_if_needed, is_xai_provider_transport,
        resolved_xai_upstream_base_url, should_attach_cli_identity_headers, XAI_API_BASE_URL,
        XAI_CHAT_PROXY_BASE_URL, XAI_CLIENT_IDENTIFIER_VALUE, XAI_TOKEN_AUTH_VALUE,
    };
    use crate::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };
    use std::collections::BTreeMap;

    fn sample_transport(
        auth_type: &str,
        auth_config: Option<&str>,
        base_url: &str,
    ) -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-xai".to_string(),
                name: "xAI".to_string(),
                provider_type: "xai".to_string(),
                website: None,
                is_active: true,
                keep_priority_on_conversion: false,
                enable_format_conversion: true,
                concurrent_limit: None,
                max_retries: None,
                proxy: None,
                request_timeout_secs: None,
                stream_first_byte_timeout_secs: None,
                config: None,
            },
            endpoint: GatewayProviderTransportEndpoint {
                id: "endpoint-xai".to_string(),
                provider_id: "provider-xai".to_string(),
                api_format: "openai:responses".to_string(),
                api_family: None,
                endpoint_kind: None,
                is_active: true,
                base_url: base_url.to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: None,
                config: None,
                format_acceptance_config: None,
                proxy: None,
            },
            key: GatewayProviderTransportKey {
                id: "key-xai".to_string(),
                provider_id: "provider-xai".to_string(),
                name: "key".to_string(),
                auth_type: auth_type.to_string(),
                is_active: true,
                api_formats: None,
                auth_type_by_format: None,
                allow_auth_channel_mismatch_formats: None,
                allowed_models: None,
                capabilities: None,
                rate_multipliers: None,
                global_priority_by_format: None,
                expires_at_unix_secs: None,
                proxy: None,
                fingerprint: None,
                upstream_metadata: None,
                decrypted_api_key: "access-token".to_string(),
                decrypted_auth_config: auth_config.map(ToOwned::to_owned),
            },
        }
    }

    #[test]
    fn oauth_defaults_to_cli_chat_proxy_for_responses() {
        let transport = sample_transport(
            "oauth",
            Some(r#"{"refresh_token":"rt","using_api":false}"#),
            XAI_CHAT_PROXY_BASE_URL,
        );
        assert!(is_xai_provider_transport(&transport));
        assert_eq!(
            resolved_xai_upstream_base_url(&transport, "openai:responses").as_deref(),
            Some(XAI_CHAT_PROXY_BASE_URL)
        );
        assert!(should_attach_cli_identity_headers(
            &transport,
            "openai:responses"
        ));
    }

    #[test]
    fn compact_and_using_api_stay_on_official_api() {
        let oauth = sample_transport(
            "oauth",
            Some(r#"{"refresh_token":"rt","using_api":false}"#),
            XAI_CHAT_PROXY_BASE_URL,
        );
        assert_eq!(
            resolved_xai_upstream_base_url(&oauth, "openai:responses:compact").as_deref(),
            Some(XAI_API_BASE_URL)
        );
        assert!(!should_attach_cli_identity_headers(
            &oauth,
            "openai:responses:compact"
        ));

        let api_key = sample_transport(
            "oauth",
            Some(r#"{"using_api":true}"#),
            XAI_CHAT_PROXY_BASE_URL,
        );
        assert_eq!(
            resolved_xai_upstream_base_url(&api_key, "openai:responses").as_deref(),
            Some(XAI_API_BASE_URL)
        );
        assert!(!should_attach_cli_identity_headers(
            &api_key,
            "openai:responses"
        ));
    }

    #[test]
    fn media_routing_and_cli_headers_follow_auth_and_base_url() {
        for api_format in ["openai:image", "openai:video"] {
            for stored in ["", XAI_API_BASE_URL, XAI_CHAT_PROXY_BASE_URL] {
                for (auth_type, config, expected) in [
                    (
                        "oauth",
                        Some(r#"{"refresh_token":"rt","using_api":false}"#),
                        XAI_CHAT_PROXY_BASE_URL,
                    ),
                    ("oauth", Some(r#"{"using_api":true}"#), XAI_API_BASE_URL),
                    ("bearer", None, XAI_API_BASE_URL),
                ] {
                    let transport = sample_transport(auth_type, config, stored);
                    assert_eq!(
                        resolved_xai_upstream_base_url(&transport, api_format).as_deref(),
                        Some(expected)
                    );
                    assert_eq!(
                        should_attach_cli_identity_headers(&transport, api_format),
                        expected == XAI_CHAT_PROXY_BASE_URL
                    );
                }
            }
            let custom = sample_transport("oauth", None, "https://custom.example/v1");
            assert_eq!(
                resolved_xai_upstream_base_url(&custom, api_format).as_deref(),
                Some("https://custom.example/v1")
            );
            assert!(!should_attach_cli_identity_headers(&custom, api_format));
        }
    }

    #[test]
    fn bearer_without_refresh_uses_official_api() {
        let transport = sample_transport("bearer", None, XAI_CHAT_PROXY_BASE_URL);
        assert_eq!(
            resolved_xai_upstream_base_url(&transport, "openai:responses").as_deref(),
            Some(XAI_API_BASE_URL)
        );
    }

    #[test]
    fn cli_headers_do_not_override_existing_values() {
        let transport = sample_transport(
            "oauth",
            Some(r#"{"refresh_token":"rt"}"#),
            XAI_CHAT_PROXY_BASE_URL,
        );
        let mut headers = BTreeMap::from([(
            "x-grok-client-identifier".to_string(),
            "custom-client".to_string(),
        )]);
        insert_cli_identity_headers_if_needed(&transport, "openai:responses", &mut headers);
        assert_eq!(
            headers.get("x-grok-client-identifier").map(String::as_str),
            Some("custom-client")
        );
        assert_eq!(
            headers.get("x-xai-token-auth").map(String::as_str),
            Some(XAI_TOKEN_AUTH_VALUE)
        );
        assert_eq!(
            headers.get("x-authenticateresponse").map(String::as_str),
            Some("authenticate-response")
        );
        assert_ne!(
            headers.get("x-grok-client-identifier").map(String::as_str),
            Some(XAI_CLIENT_IDENTIFIER_VALUE)
        );
    }

    #[test]
    fn extracts_user_id_from_user_payload_and_auth_config_sub() {
        use super::{
            extract_xai_user_id_from_auth_config, extract_xai_user_id_from_value, xai_auth_uses_api,
        };
        use serde_json::json;

        assert_eq!(
            extract_xai_user_id_from_value(&json!({"userId": "user-42"})).as_deref(),
            Some("user-42")
        );
        assert_eq!(
            extract_xai_user_id_from_auth_config(Some(r#"{"sub":"subject-1"}"#)).as_deref(),
            Some("subject-1")
        );
        assert!(!xai_auth_uses_api(
            "oauth",
            Some(r#"{"refresh_token":"rt","using_api":false}"#)
        ));
        assert!(xai_auth_uses_api(
            "bearer",
            Some(r#"{"api_key":"xai-key","using_api":true}"#)
        ));
    }
}
