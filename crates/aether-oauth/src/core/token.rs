use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, PartialEq)]
pub struct OAuthTokenSet {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_type: Option<String>,
    pub scope: Option<String>,
    pub expires_at_unix_secs: Option<u64>,
    pub raw_payload: Option<Value>,
}

impl std::fmt::Debug for OAuthTokenSet {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OAuthTokenSet")
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("token_type", &self.token_type)
            .field("scope", &self.scope)
            .field("expires_at_unix_secs", &self.expires_at_unix_secs)
            .field(
                "raw_payload",
                &self.raw_payload.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl OAuthTokenSet {
    pub fn from_token_payload(payload: Value) -> Option<Self> {
        let access_token = non_empty_string(payload.get("access_token"))
            .or_else(|| non_empty_string(payload.get("accessToken")))?;
        let expires_at_unix_secs = json_u64(
            payload
                .get("expires_in")
                .or_else(|| payload.get("expiresIn")),
        )
        .map(|expires_in| current_unix_secs().saturating_add(expires_in))
        .or_else(|| {
            json_u64(
                payload
                    .get("expires_at")
                    .or_else(|| payload.get("expiresAt")),
            )
        });

        Some(Self {
            access_token,
            refresh_token: non_empty_string(
                payload
                    .get("refresh_token")
                    .or_else(|| payload.get("refreshToken")),
            ),
            token_type: non_empty_string(
                payload
                    .get("token_type")
                    .or_else(|| payload.get("tokenType")),
            ),
            scope: non_empty_string(payload.get("scope")),
            expires_at_unix_secs,
            raw_payload: Some(payload),
        })
    }

    pub fn bearer_header_value(&self) -> String {
        format!("Bearer {}", self.access_token.trim())
    }

    pub fn requires_refresh(&self, skew_secs: u64) -> bool {
        self.expires_at_unix_secs
            .map(|expires_at| current_unix_secs() >= expires_at.saturating_sub(skew_secs))
            .unwrap_or(false)
    }

    pub fn rotated_refresh_token<'a>(&'a self, existing: Option<&'a str>) -> Option<&'a str> {
        self.refresh_token
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .or_else(|| existing.map(str::trim).filter(|value| !value.is_empty()))
    }
}

pub fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|value| value.as_secs())
        .unwrap_or_default()
}

fn non_empty_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn json_u64(value: Option<&Value>) -> Option<u64> {
    match value? {
        Value::Number(number) => number.as_u64(),
        Value::String(value) => value.trim().parse::<u64>().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::OAuthTokenSet;
    use serde_json::json;

    #[test]
    fn parses_token_payload_and_preserves_refresh_token() {
        let token = OAuthTokenSet::from_token_payload(json!({
            "access_token": "access",
            "refresh_token": "refresh",
            "expires_in": 3600
        }))
        .expect("token should parse");

        assert_eq!(token.access_token, "access");
        assert_eq!(token.refresh_token.as_deref(), Some("refresh"));
        assert!(token.expires_at_unix_secs.is_some());
        assert_eq!(token.bearer_header_value(), "Bearer access");
    }

    #[test]
    fn debug_output_redacts_tokens_and_raw_payload() {
        let token = OAuthTokenSet::from_token_payload(json!({
            "access_token": "access-secret-sentinel",
            "refresh_token": "refresh-secret-sentinel",
            "provider_secret": "raw-secret-sentinel"
        }))
        .expect("token should parse");

        let debug = format!("{token:?}");
        assert!(!debug.contains("access-secret-sentinel"));
        assert!(!debug.contains("refresh-secret-sentinel"));
        assert!(!debug.contains("raw-secret-sentinel"));
        assert!(debug.contains("<redacted>"));
    }
}
