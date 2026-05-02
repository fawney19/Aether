use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use url::{form_urlencoded, Url};
use uuid::Uuid;

pub fn generate_oauth_nonce() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

pub fn generate_pkce_verifier() -> String {
    format!(
        "{}{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    )
}

pub fn pkce_s256(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

pub fn parse_oauth_callback_params(callback_url: &str) -> BTreeMap<String, String> {
    let mut merged = BTreeMap::new();
    let Ok(url) = Url::parse(callback_url.trim()) else {
        return merged;
    };

    for (key, value) in form_urlencoded::parse(url.query().unwrap_or_default().as_bytes()) {
        merged.insert(key.into_owned(), value.into_owned());
    }
    if let Some(fragment) = url.fragment() {
        for (key, value) in form_urlencoded::parse(fragment.trim_start_matches('#').as_bytes()) {
            merged.insert(key.into_owned(), value.into_owned());
        }
    }
    if let Some(code) = merged.get("code").cloned() {
        if let Some((code_part, state_part)) = code.split_once('#') {
            merged.insert("code".to_string(), code_part.to_string());
            if !merged.contains_key("state") && !state_part.is_empty() {
                let normalized_state = state_part
                    .strip_prefix("state=")
                    .unwrap_or(state_part)
                    .trim();
                if !normalized_state.is_empty() {
                    merged.insert("state".to_string(), normalized_state.to_string());
                }
            }
        }
    }

    merged
}

#[cfg(test)]
mod tests {
    use super::{parse_oauth_callback_params, pkce_s256};

    #[test]
    fn parses_query_and_fragment_callback_params() {
        let params = parse_oauth_callback_params(
            "http://localhost/callback?code=query&state=old#code=fragment&scope=email",
        );
        assert_eq!(params.get("code").map(String::as_str), Some("fragment"));
        assert_eq!(params.get("state").map(String::as_str), Some("old"));
        assert_eq!(params.get("scope").map(String::as_str), Some("email"));
    }

    #[test]
    fn extracts_state_from_code_suffix() {
        let params =
            parse_oauth_callback_params("http://localhost/callback?code=abc%23state=state-1");
        assert_eq!(params.get("code").map(String::as_str), Some("abc"));
        assert_eq!(params.get("state").map(String::as_str), Some("state-1"));
    }

    #[test]
    fn pkce_s256_is_url_safe() {
        let value = pkce_s256("verifier");
        assert!(!value.contains('+'));
        assert!(!value.contains('/'));
        assert!(!value.contains('='));
    }
}
