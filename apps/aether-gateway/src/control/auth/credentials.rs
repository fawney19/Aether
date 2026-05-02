use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Bytes;
use axum::http::Uri;
use sha2::{Digest, Sha256};
use url::form_urlencoded;

use crate::{
    ai_serving::extract_gemini_model_from_path,
    headers::{header_value_str, is_json_request},
};

use super::super::GatewayControlDecision;
use super::types::{
    GatewayCredentialBundle, GatewayCredentialCarrier, GatewayExtractedCredentials,
    GatewayPrimaryCredential, GatewayTrustedAdminHeaders, GatewayTrustedAuthHeaders,
};

pub(crate) fn extract_requested_model(
    decision: &GatewayControlDecision,
    uri: &Uri,
    headers: &http::HeaderMap,
    body: &Bytes,
) -> Option<String> {
    if decision.route_family.as_deref() == Some("gemini") {
        if let Some(model) = extract_gemini_model_from_path(uri.path()) {
            return Some(model);
        }
    }

    if !is_json_request(headers) || body.is_empty() {
        return None;
    }
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|payload| {
            payload
                .get("model")
                .and_then(|value| value.as_str())
                .map(|value| value.trim().to_string())
        })
        .filter(|value| !value.is_empty())
}

pub(super) fn extract_request_credentials(
    headers: &http::HeaderMap,
    uri: &Uri,
    auth_endpoint_signature: &str,
) -> GatewayExtractedCredentials {
    let bundle = GatewayCredentialBundle {
        authorization_bearer: header_value_str(headers, http::header::AUTHORIZATION.as_str())
            .as_deref()
            .and_then(extract_bearer_token)
            .map(ToOwned::to_owned),
        x_api_key: header_value_str(headers, "x-api-key"),
        api_key: header_value_str(headers, "api-key"),
        x_goog_api_key: header_value_str(headers, "x-goog-api-key"),
        query_key: extract_query_api_key(uri),
        cookie_header: header_value_str(headers, http::header::COOKIE.as_str()),
    };
    let trusted_headers = extract_trusted_auth_headers(headers);
    let trusted_admin_headers = extract_trusted_admin_headers(headers);
    let primary = select_primary_credential(auth_endpoint_signature, &bundle);

    GatewayExtractedCredentials {
        trusted_headers,
        trusted_admin_headers,
        bundle,
        primary,
    }
}

fn has_trusted_gateway_marker(headers: &http::HeaderMap) -> bool {
    header_value_str(headers, crate::constants::GATEWAY_HEADER)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .starts_with("rust-phase3")
}

pub(super) fn build_auth_context_cache_key(
    headers: &http::HeaderMap,
    uri: &Uri,
    auth_endpoint_signature: &str,
) -> Option<String> {
    let signature = auth_endpoint_signature.trim();
    if signature.is_empty() {
        return None;
    }

    let extracted = extract_request_credentials(headers, uri, signature);
    let bundle = extracted.bundle;
    if bundle.authorization_bearer.is_none()
        && bundle.x_api_key.is_none()
        && bundle.api_key.is_none()
        && bundle.x_goog_api_key.is_none()
        && bundle.query_key.is_none()
        && bundle.cookie_header.is_none()
    {
        return None;
    }

    Some(format!(
        "{signature}\n{}\n{}\n{}\n{}\n{}\n{}",
        bundle.authorization_bearer.unwrap_or_default(),
        bundle.x_api_key.unwrap_or_default(),
        bundle.api_key.unwrap_or_default(),
        bundle.x_goog_api_key.unwrap_or_default(),
        bundle.query_key.unwrap_or_default(),
        bundle.cookie_header.unwrap_or_default(),
    ))
}

fn extract_trusted_auth_headers(headers: &http::HeaderMap) -> Option<GatewayTrustedAuthHeaders> {
    if !has_trusted_gateway_marker(headers) {
        return None;
    }
    let user_id = header_value_str(headers, crate::constants::TRUSTED_AUTH_USER_ID_HEADER)
        .filter(|value| !value.is_empty())?;
    let api_key_id = header_value_str(headers, crate::constants::TRUSTED_AUTH_API_KEY_ID_HEADER)
        .filter(|value| !value.is_empty())?;
    let balance_remaining =
        header_value_str(headers, crate::constants::TRUSTED_AUTH_BALANCE_HEADER)
            .as_deref()
            .and_then(parse_f64_header);
    let access_allowed = header_value_str(
        headers,
        crate::constants::TRUSTED_AUTH_ACCESS_ALLOWED_HEADER,
    )
    .as_deref()
    .and_then(parse_bool_header);

    Some(GatewayTrustedAuthHeaders {
        user_id,
        api_key_id,
        balance_remaining,
        access_allowed,
    })
}

pub(super) fn extract_trusted_admin_headers(
    headers: &http::HeaderMap,
) -> Option<GatewayTrustedAdminHeaders> {
    if !has_trusted_gateway_marker(headers) {
        return None;
    }
    let user_id = header_value_str(headers, crate::constants::TRUSTED_ADMIN_USER_ID_HEADER)?
        .trim()
        .to_string();
    if user_id.is_empty() {
        return None;
    }
    let user_role = header_value_str(headers, crate::constants::TRUSTED_ADMIN_USER_ROLE_HEADER)?
        .trim()
        .to_string();
    if !user_role.eq_ignore_ascii_case("admin") {
        return None;
    }
    let session_id = header_value_str(headers, crate::constants::TRUSTED_ADMIN_SESSION_ID_HEADER)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let management_token_id = header_value_str(
        headers,
        crate::constants::TRUSTED_ADMIN_MANAGEMENT_TOKEN_ID_HEADER,
    )
    .map(|value| value.trim().to_string())
    .filter(|value| !value.is_empty());
    if session_id.is_none() && management_token_id.is_none() {
        return None;
    }

    Some(GatewayTrustedAdminHeaders {
        user_id,
        user_role: "admin".to_string(),
        session_id,
        management_token_id,
    })
}

fn select_primary_credential(
    auth_endpoint_signature: &str,
    bundle: &GatewayCredentialBundle,
) -> Option<GatewayPrimaryCredential> {
    let signature = auth_endpoint_signature.trim().to_ascii_lowercase();
    if signature.starts_with("gemini:") {
        return select_gemini_credential(bundle);
    }
    if signature.starts_with("claude:") {
        return select_claude_messages_credential(bundle);
    }
    if signature.starts_with("openai:") {
        return select_openai_credential(bundle);
    }

    select_generic_credential(bundle)
}

fn select_openai_credential(bundle: &GatewayCredentialBundle) -> Option<GatewayPrimaryCredential> {
    first_provider_api_key(
        bundle,
        &[
            GatewayCredentialCarrier::AuthorizationBearer,
            GatewayCredentialCarrier::XApiKey,
            GatewayCredentialCarrier::ApiKey,
            GatewayCredentialCarrier::XGoogApiKey,
            GatewayCredentialCarrier::QueryKey,
        ],
    )
    .or_else(|| select_cookie_credential(bundle))
}

fn select_claude_messages_credential(
    bundle: &GatewayCredentialBundle,
) -> Option<GatewayPrimaryCredential> {
    first_provider_api_key(
        bundle,
        &[
            GatewayCredentialCarrier::XApiKey,
            GatewayCredentialCarrier::ApiKey,
            GatewayCredentialCarrier::AuthorizationBearer,
        ],
    )
    .or_else(|| first_bearer_token(bundle))
    .or_else(|| select_cookie_credential(bundle))
}

fn select_gemini_credential(bundle: &GatewayCredentialBundle) -> Option<GatewayPrimaryCredential> {
    first_provider_api_key(
        bundle,
        &[
            GatewayCredentialCarrier::QueryKey,
            GatewayCredentialCarrier::XGoogApiKey,
            GatewayCredentialCarrier::XApiKey,
            GatewayCredentialCarrier::ApiKey,
        ],
    )
    .or_else(|| first_bearer_token(bundle))
    .or_else(|| select_cookie_credential(bundle))
}

fn select_generic_credential(bundle: &GatewayCredentialBundle) -> Option<GatewayPrimaryCredential> {
    first_bearer_token(bundle)
        .or_else(|| {
            first_provider_api_key(
                bundle,
                &[
                    GatewayCredentialCarrier::XApiKey,
                    GatewayCredentialCarrier::ApiKey,
                    GatewayCredentialCarrier::XGoogApiKey,
                    GatewayCredentialCarrier::QueryKey,
                ],
            )
        })
        .or_else(|| select_cookie_credential(bundle))
}

fn first_provider_api_key(
    bundle: &GatewayCredentialBundle,
    carriers: &[GatewayCredentialCarrier],
) -> Option<GatewayPrimaryCredential> {
    for carrier in carriers {
        if let Some(raw) = credential_value(bundle, *carrier) {
            return Some(GatewayPrimaryCredential::ProviderApiKey {
                raw,
                carrier: *carrier,
            });
        }
    }
    None
}

fn first_bearer_token(bundle: &GatewayCredentialBundle) -> Option<GatewayPrimaryCredential> {
    credential_value(bundle, GatewayCredentialCarrier::AuthorizationBearer).map(|raw| {
        GatewayPrimaryCredential::BearerToken {
            raw,
            carrier: GatewayCredentialCarrier::AuthorizationBearer,
        }
    })
}

fn select_cookie_credential(bundle: &GatewayCredentialBundle) -> Option<GatewayPrimaryCredential> {
    credential_value(bundle, GatewayCredentialCarrier::CookieHeader).map(|raw| {
        GatewayPrimaryCredential::CookieHeader {
            raw,
            carrier: GatewayCredentialCarrier::CookieHeader,
        }
    })
}

fn credential_value(
    bundle: &GatewayCredentialBundle,
    carrier: GatewayCredentialCarrier,
) -> Option<String> {
    match carrier {
        GatewayCredentialCarrier::AuthorizationBearer => bundle.authorization_bearer.clone(),
        GatewayCredentialCarrier::XApiKey => bundle.x_api_key.clone(),
        GatewayCredentialCarrier::ApiKey => bundle.api_key.clone(),
        GatewayCredentialCarrier::XGoogApiKey => bundle.x_goog_api_key.clone(),
        GatewayCredentialCarrier::QueryKey => bundle.query_key.clone(),
        GatewayCredentialCarrier::CookieHeader => bundle.cookie_header.clone(),
    }
}

fn extract_query_api_key(uri: &Uri) -> Option<String> {
    let query = uri.query()?;
    form_urlencoded::parse(query.as_bytes())
        .find(|(key, value)| key == "key" && !value.trim().is_empty())
        .map(|(_, value)| value.into_owned())
}

fn extract_bearer_token(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    let (scheme, token) = trimmed.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

pub(super) fn hash_api_key(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub(super) fn contains_string(items: &[String], target: &str) -> bool {
    items
        .iter()
        .any(|item| item.trim().eq_ignore_ascii_case(target.trim()))
}

pub(super) fn parse_bool_header(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

pub(super) fn parse_f64_header(value: &str) -> Option<f64> {
    value.trim().parse::<f64>().ok()
}

pub(super) fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::{
        build_auth_context_cache_key, extract_request_credentials, GatewayCredentialCarrier,
        GatewayPrimaryCredential, GatewayTrustedAdminHeaders, GatewayTrustedAuthHeaders,
    };
    use axum::http::{self, Uri};

    fn uri(path: &str) -> Uri {
        path.parse().expect("uri should parse")
    }

    #[test]
    fn selects_openai_bearer_as_provider_api_key() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            "Bearer sk-openai".parse().unwrap(),
        );

        let extracted =
            extract_request_credentials(&headers, &uri("/v1/chat/completions"), "openai:chat");
        assert_eq!(
            extracted.primary,
            Some(GatewayPrimaryCredential::ProviderApiKey {
                raw: "sk-openai".to_string(),
                carrier: GatewayCredentialCarrier::AuthorizationBearer,
            })
        );
    }

    #[test]
    fn prefers_claude_chat_x_api_key_over_bearer() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            "Bearer cli-token".parse().unwrap(),
        );
        headers.insert("x-api-key", "claude-key".parse().unwrap());

        let extracted =
            extract_request_credentials(&headers, &uri("/v1/messages"), "claude:messages");
        assert_eq!(
            extracted.primary,
            Some(GatewayPrimaryCredential::ProviderApiKey {
                raw: "claude-key".to_string(),
                carrier: GatewayCredentialCarrier::XApiKey,
            })
        );
    }

    #[test]
    fn selects_claude_cli_bearer_as_provider_api_key() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            "Bearer cli-token".parse().unwrap(),
        );

        let extracted =
            extract_request_credentials(&headers, &uri("/v1/messages"), "claude:messages");
        assert_eq!(
            extracted.primary,
            Some(GatewayPrimaryCredential::ProviderApiKey {
                raw: "cli-token".to_string(),
                carrier: GatewayCredentialCarrier::AuthorizationBearer,
            })
        );
    }

    #[test]
    fn prefers_gemini_query_key_over_header_key() {
        let mut headers = http::HeaderMap::new();
        headers.insert("x-goog-api-key", "gemini-header".parse().unwrap());

        let extracted = extract_request_credentials(
            &headers,
            &uri("/v1beta/models?key=gemini-query"),
            "gemini:generate_content",
        );
        assert_eq!(
            extracted.primary,
            Some(GatewayPrimaryCredential::ProviderApiKey {
                raw: "gemini-query".to_string(),
                carrier: GatewayCredentialCarrier::QueryKey,
            })
        );
    }

    #[test]
    fn extracts_cookie_as_fallback_credential() {
        let mut headers = http::HeaderMap::new();
        headers.insert(http::header::COOKIE, "session=abc123".parse().unwrap());

        let extracted =
            extract_request_credentials(&headers, &uri("/v1/chat/completions"), "internal:session");
        assert_eq!(
            extracted.primary,
            Some(GatewayPrimaryCredential::CookieHeader {
                raw: "session=abc123".to_string(),
                carrier: GatewayCredentialCarrier::CookieHeader,
            })
        );
    }

    #[test]
    fn cache_key_includes_cookie_header() {
        let mut headers = http::HeaderMap::new();
        headers.insert(http::header::COOKIE, "session=abc123".parse().unwrap());

        let cache_key = build_auth_context_cache_key(
            &headers,
            &uri("/v1/chat/completions"),
            "internal:session",
        )
        .expect("cache key should exist");
        assert!(cache_key.contains("session=abc123"));
    }

    #[test]
    fn extracts_trusted_auth_headers() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            crate::constants::GATEWAY_HEADER,
            "rust-phase3b".parse().unwrap(),
        );
        headers.insert(
            crate::constants::TRUSTED_AUTH_USER_ID_HEADER,
            "user-1".parse().unwrap(),
        );
        headers.insert(
            crate::constants::TRUSTED_AUTH_API_KEY_ID_HEADER,
            "key-1".parse().unwrap(),
        );
        headers.insert(
            crate::constants::TRUSTED_AUTH_BALANCE_HEADER,
            "1.5".parse().unwrap(),
        );
        headers.insert(
            crate::constants::TRUSTED_AUTH_ACCESS_ALLOWED_HEADER,
            "true".parse().unwrap(),
        );

        let extracted =
            extract_request_credentials(&headers, &uri("/v1/chat/completions"), "openai:chat");
        assert_eq!(
            extracted.trusted_headers,
            Some(GatewayTrustedAuthHeaders {
                user_id: "user-1".to_string(),
                api_key_id: "key-1".to_string(),
                balance_remaining: Some(1.5),
                access_allowed: Some(true),
            })
        );
        assert_eq!(extracted.trusted_admin_headers, None);
    }

    #[test]
    fn ignores_trusted_auth_headers_without_gateway_marker() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            crate::constants::TRUSTED_AUTH_USER_ID_HEADER,
            "user-1".parse().unwrap(),
        );
        headers.insert(
            crate::constants::TRUSTED_AUTH_API_KEY_ID_HEADER,
            "key-1".parse().unwrap(),
        );

        let extracted =
            extract_request_credentials(&headers, &uri("/v1/chat/completions"), "openai:chat");
        assert_eq!(extracted.trusted_headers, None);
    }

    #[test]
    fn extracts_trusted_admin_headers() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            crate::constants::GATEWAY_HEADER,
            "rust-phase3b".parse().unwrap(),
        );
        headers.insert(
            crate::constants::TRUSTED_ADMIN_USER_ID_HEADER,
            "admin-user-1".parse().unwrap(),
        );
        headers.insert(
            crate::constants::TRUSTED_ADMIN_USER_ROLE_HEADER,
            "admin".parse().unwrap(),
        );
        headers.insert(
            crate::constants::TRUSTED_ADMIN_SESSION_ID_HEADER,
            "sess-1".parse().unwrap(),
        );

        let extracted = extract_request_credentials(
            &headers,
            &uri("/api/admin/endpoints/health/api-formats"),
            "admin:endpoints_health",
        );
        assert_eq!(
            extracted.trusted_admin_headers,
            Some(GatewayTrustedAdminHeaders {
                user_id: "admin-user-1".to_string(),
                user_role: "admin".to_string(),
                session_id: Some("sess-1".to_string()),
                management_token_id: None,
            })
        );
    }

    #[test]
    fn ignores_trusted_admin_headers_without_gateway_marker() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            crate::constants::TRUSTED_ADMIN_USER_ID_HEADER,
            "admin-user-1".parse().unwrap(),
        );
        headers.insert(
            crate::constants::TRUSTED_ADMIN_USER_ROLE_HEADER,
            "admin".parse().unwrap(),
        );
        headers.insert(
            crate::constants::TRUSTED_ADMIN_SESSION_ID_HEADER,
            "sess-1".parse().unwrap(),
        );

        let extracted = extract_request_credentials(
            &headers,
            &uri("/api/admin/endpoints/health/api-formats"),
            "admin:endpoints_health",
        );
        assert_eq!(extracted.trusted_admin_headers, None);
    }
}
