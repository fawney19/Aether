use super::super::provisioning::build_provider_oauth_auth_config_from_token_payload;
use super::super::state::json_u64_value;
use base64::{
    engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD},
    Engine as _,
};
use serde_json::{json, Map, Value};

fn decode_base64_url_part(value: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD
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
        .ok()
}

fn decode_unverified_jwt_json_part(part: &str) -> Option<Map<String, Value>> {
    let bytes = decode_base64_url_part(part)?;
    serde_json::from_slice::<Value>(&bytes)
        .ok()?
        .as_object()
        .cloned()
}

pub(super) fn looks_like_access_token(token: &str) -> bool {
    let parts = token.trim().split('.').collect::<Vec<_>>();
    if parts.len() != 3 || parts.iter().any(|part| part.is_empty()) {
        return false;
    }

    let Some(header) = decode_unverified_jwt_json_part(parts[0]) else {
        return false;
    };
    let Some(payload) = decode_unverified_jwt_json_part(parts[1]) else {
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

pub(super) fn build_codex_access_token_import_auth_config(
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
        build_provider_oauth_auth_config_from_token_payload("codex", &token_payload);

    let refresh_token = refresh_token
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(refresh_token) = refresh_token {
        auth_config.insert("refresh_token".to_string(), json!(refresh_token));
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
        build_codex_access_token_import_auth_config, decode_access_token_expires_at,
        looks_like_access_token, normalize_single_import_tokens,
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
    fn builds_codex_temporary_auth_config_from_access_token() {
        let token = unsigned_jwt(json!({
            "exp": 2_000_000_000u64,
            "https://api.openai.com/profile": {
                "email": "u@example.com"
            },
        }));

        let (auth_config, expires_at) =
            build_codex_access_token_import_auth_config(&token, None, None, None);

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

        let (auth_config, expires_at) =
            build_codex_access_token_import_auth_config(&token, None, Some(2_100_000_000), None);

        assert_eq!(expires_at, Some(2_100_000_000));
        assert_eq!(
            auth_config.get("expires_at"),
            Some(&json!(2_100_000_000u64))
        );
    }
}
