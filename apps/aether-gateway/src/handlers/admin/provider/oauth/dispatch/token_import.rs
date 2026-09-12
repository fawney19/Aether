use super::super::provisioning::build_provider_oauth_auth_config_from_token_payload;
use super::super::state::json_u64_value;
use base64::{
    engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD},
    Engine as _,
};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

const MAX_UNVERIFIED_JWT_PART_BYTES: usize = 64 * 1024;

fn decode_base64_url_part(value: &str) -> Option<Vec<u8>> {
    if value.len()
        > crate::execution_runtime::transport::maximum_base64_len_for_decoded_limit(
            MAX_UNVERIFIED_JWT_PART_BYTES,
        )
    {
        return None;
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(value.as_bytes())
        .or_else(|_| URL_SAFE.decode(value.as_bytes()))
        .or_else(|_| {
            let mut padded = value.to_string();
            let remainder = padded.len() % 4;
            if remainder != 0 {
                padded.extend(std::iter::repeat_n('=', 4 - remainder));
            }
            URL_SAFE.decode(padded.as_bytes())
        })
        .ok()?;
    (bytes.len() <= MAX_UNVERIFIED_JWT_PART_BYTES).then_some(bytes)
}

fn decode_unverified_jwt_json_part(part: &str) -> Option<Map<String, Value>> {
    let bytes = decode_base64_url_part(part)?;
    serde_json::from_slice::<Value>(&bytes)
        .ok()?
        .as_object()
        .cloned()
}

pub(super) fn looks_like_access_token(token: &str) -> bool {
    let mut parts = token.trim().split('.');
    let Some(header_part) = parts.next().filter(|part| !part.is_empty()) else {
        return false;
    };
    let Some(payload_part) = parts.next().filter(|part| !part.is_empty()) else {
        return false;
    };
    let Some(_signature_part) = parts.next().filter(|part| !part.is_empty()) else {
        return false;
    };
    // A JWT has exactly three dot-separated parts. Avoid collecting all
    // attacker-controlled segments into a temporary Vec just to reject extras.
    if parts.next().is_some() {
        return false;
    }

    let Some(header) = decode_unverified_jwt_json_part(header_part) else {
        return false;
    };
    let Some(payload) = decode_unverified_jwt_json_part(payload_part) else {
        return false;
    };

    let token_type = header
        .get("typ")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !token_type.is_empty() && token_type != "jwt" && token_type != "at+jwt" {
        return false;
    }

    ["exp", "aud", "iss", "scope", "scp"]
        .iter()
        .any(|field| payload.contains_key(*field))
}

pub(super) fn normalize_single_import_tokens(
    refresh_token: Option<&str>,
    access_token: Option<&str>,
) -> (Option<String>, Option<String>) {
    let mut refresh_token = refresh_token
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let mut access_token = access_token
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    if access_token.is_none()
        && refresh_token
            .as_deref()
            .is_some_and(looks_like_access_token)
    {
        access_token = refresh_token.take();
    }

    (refresh_token, access_token)
}

pub(super) fn normalize_provider_import_tokens(
    provider_type: &str,
    refresh_token: Option<&str>,
    access_token: Option<&str>,
) -> (Option<String>, Option<String>) {
    let provider_type = provider_type.trim().to_ascii_lowercase();
    let refresh_token = refresh_token
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let access_token = access_token
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    if provider_type == "grok" {
        return (None, access_token.or(refresh_token));
    }
    if provider_type == "claude_code" {
        if access_token.is_none() && refresh_token.as_deref().is_some_and(is_claude_access_token) {
            return (None, refresh_token);
        }
        return (refresh_token, access_token);
    }

    normalize_single_import_tokens(refresh_token.as_deref(), access_token.as_deref())
}

pub(super) fn import_tokens_from_raw_token(token: &str) -> (Option<String>, Option<String>) {
    if looks_like_access_token(token) {
        (None, Some(token.trim().to_string()))
    } else {
        (Some(token.trim().to_string()), None)
    }
}

pub(super) fn decode_access_token_expires_at(access_token: &str) -> Option<u64> {
    let payload = access_token.trim().split('.').nth(1)?;
    let claims = decode_unverified_jwt_json_part(payload)?;
    json_u64_value(claims.get("exp"))
}

fn normalize_import_header_name(raw: &str) -> Option<String> {
    let value = raw.trim().to_ascii_lowercase();
    if value.is_empty()
        || matches!(
            value.as_str(),
            "host"
                | "content-length"
                | "connection"
                | "keep-alive"
                | "proxy-authenticate"
                | "proxy-authorization"
                | "proxy-connection"
                | "te"
                | "trailer"
                | "transfer-encoding"
                | "upgrade"
        )
    {
        return None;
    }
    http::header::HeaderName::from_bytes(value.as_bytes())
        .ok()
        .map(|name| name.as_str().to_string())
}

fn normalize_import_header_value(value: &Value) -> Option<String> {
    let value = match value {
        Value::String(raw) => raw.trim().to_string(),
        Value::Number(raw) => raw.to_string(),
        Value::Bool(raw) => raw.to_string(),
        _ => return None,
    };
    if value.is_empty() || http::header::HeaderValue::from_str(&value).is_err() {
        return None;
    }
    Some(value)
}

pub(super) fn normalize_provider_oauth_import_headers(
    value: Option<&Value>,
) -> Option<BTreeMap<String, String>> {
    let object = value?.as_object()?;
    let mut headers = BTreeMap::new();
    for (raw_key, raw_value) in object {
        let Some(key) = normalize_import_header_name(raw_key) else {
            continue;
        };
        let Some(value) = normalize_import_header_value(raw_value) else {
            continue;
        };
        headers.insert(key, value);
    }
    (!headers.is_empty()).then_some(headers)
}

pub(super) fn normalize_provider_oauth_import_headers_from_object(
    object: &Map<String, Value>,
) -> Option<BTreeMap<String, String>> {
    normalize_provider_oauth_import_headers(
        object
            .get("headers")
            .or_else(|| object.get("request_headers"))
            .or_else(|| object.get("requestHeaders"))
            .or_else(|| object.get("header_overrides"))
            .or_else(|| object.get("headerOverrides"))
            .or_else(|| object.get("extra_headers"))
            .or_else(|| object.get("extraHeaders")),
    )
}

pub(super) fn provider_oauth_import_authorization_bearer_token(
    headers: Option<&BTreeMap<String, String>>,
) -> Option<String> {
    let value = headers?.get("authorization")?.trim();
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then(|| token.to_string())
}

pub(super) fn provider_oauth_import_authorization_bearer_token_from_object(
    object: &Map<String, Value>,
) -> Option<String> {
    let headers = normalize_provider_oauth_import_headers_from_object(object)?;
    provider_oauth_import_authorization_bearer_token(Some(&headers))
}

pub(super) fn provider_type_supports_access_token_import(provider_type: &str) -> bool {
    matches!(
        provider_type.trim().to_ascii_lowercase().as_str(),
        "claude_code" | "codex" | "chatgpt_web" | "grok"
    )
}

pub(super) fn is_claude_access_token(value: &str) -> bool {
    value.trim().starts_with("sk-ant-oat")
}

pub(super) fn is_claude_session_key(value: &str) -> bool {
    value.trim().starts_with("sk-ant-sid")
}

pub(super) fn flatten_claude_code_credentials_payload(payload: &mut Map<String, Value>) {
    let nested = payload
        .get("claudeAiOauth")
        .or_else(|| payload.get("claude_ai_oauth"))
        .and_then(Value::as_object)
        .cloned();
    let Some(nested) = nested else {
        return;
    };

    for (target, aliases) in [
        ("access_token", &["access_token", "accessToken"][..]),
        ("refresh_token", &["refresh_token", "refreshToken"][..]),
        ("scopes", &["scopes"][..]),
        (
            "subscription_type",
            &["subscription_type", "subscriptionType"][..],
        ),
        ("rate_limit_tier", &["rate_limit_tier", "rateLimitTier"][..]),
        (
            "organization_uuid",
            &["organization_uuid", "organizationUuid", "org_uuid"][..],
        ),
    ] {
        if payload.contains_key(target) {
            continue;
        }
        if let Some(value) = aliases.iter().find_map(|key| nested.get(*key)).cloned() {
            payload.insert(target.to_string(), value);
        }
    }

    if !payload.contains_key("expires_at") {
        if let Some(expires_at) = json_u64_value(nested.get("expires_at")) {
            payload.insert("expires_at".to_string(), json!(expires_at));
        } else if let Some(expires_at_ms) = json_u64_value(nested.get("expiresAt")) {
            payload.insert("expires_at".to_string(), json!(expires_at_ms / 1_000));
        }
    }
}

pub(super) fn validate_claude_access_token_import(
    access_token: &str,
    imported_expires_at: Option<u64>,
    now_unix_secs: u64,
) -> Result<(), &'static str> {
    if !is_claude_access_token(access_token) {
        return Err("Claude Access Token 格式无效，请导入 sk-ant-oat 凭据");
    }
    if imported_expires_at.is_none_or(|expires_at| expires_at <= now_unix_secs) {
        return Err(
            "Claude Access Token 单独导入必须提供有效的未来 expires_at；建议导入完整 Claude credentials 或 Refresh Token",
        );
    }
    Ok(())
}

pub(super) fn build_provider_access_token_import_auth_config(
    provider_type: &str,
    access_token: &str,
    refresh_token: Option<&str>,
    imported_expires_at: Option<u64>,
    refresh_error: Option<&str>,
) -> (Map<String, Value>, Option<u64>) {
    let token_payload = json!({
        "access_token": access_token,
        "token_type": "Bearer",
    });
    let (mut auth_config, _, _, _) =
        build_provider_oauth_auth_config_from_token_payload(provider_type, &token_payload);

    let refresh_token = refresh_token
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(refresh_token) = refresh_token {
        auth_config.insert("refresh_token".to_string(), json!(refresh_token));
    }

    if provider_type.trim().eq_ignore_ascii_case("grok") {
        auth_config.insert("sso_token".to_string(), json!(access_token));
        auth_config.insert("auth_method".to_string(), json!("sso_token"));
    }

    auth_config.insert(
        "access_token_import_temporary".to_string(),
        json!(refresh_token.is_none()),
    );

    if let Some(expires_at) = decode_access_token_expires_at(access_token).or(imported_expires_at) {
        auth_config.insert("expires_at".to_string(), json!(expires_at));
    }
    if let Some(refresh_error) = refresh_error
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        auth_config.insert(
            "refresh_token_import_error".to_string(),
            json!(refresh_error),
        );
    }

    let expires_at = auth_config.get("expires_at").and_then(Value::as_u64);
    (auth_config, expires_at)
}

#[cfg(test)]
mod tests {
    use super::{
        build_provider_access_token_import_auth_config, decode_access_token_expires_at,
        flatten_claude_code_credentials_payload, looks_like_access_token,
        normalize_provider_import_tokens, normalize_provider_oauth_import_headers,
        normalize_single_import_tokens, provider_oauth_import_authorization_bearer_token,
        validate_claude_access_token_import,
    };
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use serde_json::json;

    fn unsigned_jwt(payload: serde_json::Value) -> String {
        let header = json!({"alg": "none", "typ": "JWT"});
        let encode = |value: serde_json::Value| {
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&value).expect("jwt json should serialize"))
        };
        format!("{}.{}.signature", encode(header), encode(payload))
    }

    #[test]
    fn detects_plain_jwt_access_token() {
        let token = unsigned_jwt(json!({
            "iss": "https://auth.openai.com",
            "aud": ["https://api.openai.com/v1"],
            "exp": 2_000_000_000u64,
        }));

        assert!(looks_like_access_token(&token));
        let (refresh_token, access_token) = normalize_single_import_tokens(Some(&token), None);
        assert!(refresh_token.is_none());
        assert_eq!(access_token.as_deref(), Some(token.as_str()));
    }

    #[test]
    fn rejects_jwt_with_many_extra_segments_without_collecting() {
        let token = unsigned_jwt(json!({"exp": 2_000_000_000u64}));
        let oversized = format!("{token}.{}", "x.".repeat(4096));
        assert!(!looks_like_access_token(&oversized));
    }

    #[test]
    fn builds_codex_temporary_auth_config_from_access_token() {
        let token = unsigned_jwt(json!({
            "exp": 2_000_000_000u64,
            "https://api.openai.com/profile": {
                "email": "u@example.com"
            },
        }));

        let (auth_config, expires_at) =
            build_provider_access_token_import_auth_config("codex", &token, None, None, None);

        assert_eq!(expires_at, Some(2_000_000_000));
        assert_eq!(decode_access_token_expires_at(&token), Some(2_000_000_000));
        assert_eq!(auth_config.get("email"), Some(&json!("u@example.com")));
        assert_eq!(
            auth_config.get("access_token_import_temporary"),
            Some(&json!(true))
        );
        assert!(auth_config.get("refresh_token").is_none());
    }

    #[test]
    fn builds_codex_auth_config_with_imported_expires_at_when_token_has_no_exp() {
        let token = unsigned_jwt(json!({
            "iss": "https://auth.openai.com",
            "aud": ["https://api.openai.com/v1"]
        }));

        let (auth_config, expires_at) = build_provider_access_token_import_auth_config(
            "codex",
            &token,
            None,
            Some(2_100_000_000),
            None,
        );

        assert_eq!(expires_at, Some(2_100_000_000));
        assert_eq!(
            auth_config.get("expires_at"),
            Some(&json!(2_100_000_000u64))
        );
    }

    #[test]
    fn normalizes_import_header_overrides_for_auth_headers() {
        let headers = normalize_provider_oauth_import_headers(Some(&json!({
            "Authorization": " Bearer session-token ",
            "X-Feature": true,
            "Host": "evil.example",
            "X-Bad": "line\nbreak",
        })))
        .expect("headers should normalize");

        assert_eq!(
            headers.get("authorization"),
            Some(&"Bearer session-token".to_string())
        );
        assert_eq!(headers.get("x-feature"), Some(&"true".to_string()));
        assert!(!headers.contains_key("host"));
        assert!(!headers.contains_key("x-bad"));
    }

    #[test]
    fn extracts_import_authorization_header_bearer_token() {
        let headers = normalize_provider_oauth_import_headers(Some(&json!({
            "Authorization": "Bearer at-session-token",
        })))
        .expect("headers should normalize");

        assert_eq!(
            provider_oauth_import_authorization_bearer_token(Some(&headers)).as_deref(),
            Some("at-session-token")
        );
    }

    #[test]
    fn builds_chatgpt_web_temporary_auth_config_from_access_token() {
        let token = unsigned_jwt(json!({
            "exp": 2_000_000_000u64,
            "https://api.openai.com/profile": {
                "email": "image@example.com"
            },
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acct-image-123"
            },
        }));

        let (auth_config, expires_at) =
            build_provider_access_token_import_auth_config("chatgpt_web", &token, None, None, None);

        assert_eq!(expires_at, Some(2_000_000_000));
        assert_eq!(
            auth_config.get("provider_type"),
            Some(&json!("chatgpt_web"))
        );
        assert_eq!(auth_config.get("email"), Some(&json!("image@example.com")));
        assert_eq!(
            auth_config.get("account_id"),
            Some(&json!("acct-image-123"))
        );
        assert_eq!(
            auth_config.get("access_token_import_temporary"),
            Some(&json!(true))
        );
    }

    #[test]
    fn normalize_grok_import_treats_opaque_session_as_access_token() {
        let (refresh_token, access_token) =
            normalize_provider_import_tokens("grok", Some("sso_session_token"), None);
        assert!(refresh_token.is_none());
        assert_eq!(access_token.as_deref(), Some("sso_session_token"));
    }

    #[test]
    fn builds_grok_auth_config_from_session_token() {
        let (auth_config, expires_at) = build_provider_access_token_import_auth_config(
            "grok",
            "sso_session_token",
            None,
            Some(2_200_000_000),
            None,
        );

        assert_eq!(expires_at, Some(2_200_000_000));
        assert_eq!(
            auth_config.get("sso_token"),
            Some(&json!("sso_session_token"))
        );
        assert_eq!(auth_config.get("auth_method"), Some(&json!("sso_token")));
        assert_eq!(
            auth_config.get("expires_at"),
            Some(&json!(2_200_000_000u64))
        );
    }

    #[test]
    fn flattens_only_claude_ai_oauth_credentials_and_converts_expiry_ms() {
        let mut payload = json!({
            "claudeAiOauth": {
                "accessToken": "sk-ant-oat01-access",
                "refreshToken": "sk-ant-ort01-refresh",
                "expiresAt": 2_100_000_000_123u64,
                "scopes": ["user:profile"],
                "subscriptionType": "pro",
                "rateLimitTier": "tier_1",
                "organizationUuid": "org-123"
            },
            "mcpOAuth": {
                "accessToken": "must-not-be-imported"
            }
        })
        .as_object()
        .cloned()
        .expect("payload should be an object");

        flatten_claude_code_credentials_payload(&mut payload);

        assert_eq!(
            payload.get("access_token"),
            Some(&json!("sk-ant-oat01-access"))
        );
        assert_eq!(
            payload.get("refresh_token"),
            Some(&json!("sk-ant-ort01-refresh"))
        );
        assert_eq!(payload.get("expires_at"), Some(&json!(2_100_000_000u64)));
        assert_eq!(payload.get("organization_uuid"), Some(&json!("org-123")));
        assert_ne!(
            payload.get("access_token"),
            Some(&json!("must-not-be-imported"))
        );
    }

    #[test]
    fn validates_claude_access_token_prefix_and_future_expiry() {
        assert!(validate_claude_access_token_import(
            "sk-ant-oat01-access",
            Some(2_100_000_000),
            2_000_000_000,
        )
        .is_ok());
        assert!(validate_claude_access_token_import(
            "arbitrary-token",
            Some(2_100_000_000),
            2_000_000_000,
        )
        .is_err());
        assert!(validate_claude_access_token_import(
            "sk-ant-oat01-expired",
            Some(1_900_000_000),
            2_000_000_000,
        )
        .is_err());
    }
}
