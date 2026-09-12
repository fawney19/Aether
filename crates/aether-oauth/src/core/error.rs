use aether_contracts::redact_url_for_debug;
use serde_json::{json, Value};

const OAUTH_ERROR_BODY_EXCERPT_CHARS: usize = 500;

pub enum OAuthError {
    UnsupportedProvider(String),
    InvalidRequest(String),
    InvalidState,
    // `body_excerpt` remains available to trusted callers for status
    // classification, but must not be rendered by the generic Error/Debug
    // paths: OAuth servers sometimes echo access tokens, authorization codes,
    // assertions, or client credentials in an error response.
    HttpStatus {
        status_code: u16,
        body_excerpt: String,
    },
    InvalidResponse(String),
    Transport(String),
    Storage(String),
    EncryptionUnavailable,
}

impl std::fmt::Display for OAuthError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedProvider(detail) => write!(
                formatter,
                "unsupported oauth provider: {}",
                redact_oauth_error_detail(detail)
            ),
            Self::InvalidRequest(detail) => write!(
                formatter,
                "invalid oauth request: {}",
                redact_oauth_error_detail(detail)
            ),
            Self::InvalidState => formatter.write_str("oauth state is invalid or expired"),
            Self::HttpStatus { status_code, .. } => {
                write!(formatter, "oauth provider returned HTTP {status_code}")
            }
            Self::InvalidResponse(detail) => write!(
                formatter,
                "oauth provider returned invalid response: {}",
                redact_oauth_error_detail(detail)
            ),
            Self::Transport(detail) => write!(
                formatter,
                "oauth transport failed: {}",
                redact_oauth_error_detail(detail)
            ),
            Self::Storage(detail) => write!(
                formatter,
                "oauth storage failed: {}",
                redact_oauth_error_detail(detail)
            ),
            Self::EncryptionUnavailable => formatter.write_str("oauth encryption failed"),
        }
    }
}

impl std::error::Error for OAuthError {}

impl std::fmt::Debug for OAuthError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedProvider(_) => formatter
                .debug_tuple("UnsupportedProvider")
                .field(&"[REDACTED]")
                .finish(),
            Self::InvalidRequest(_) => formatter
                .debug_tuple("InvalidRequest")
                .field(&"[REDACTED]")
                .finish(),
            Self::InvalidState => formatter.write_str("InvalidState"),
            Self::HttpStatus { status_code, .. } => formatter
                .debug_struct("HttpStatus")
                .field("status_code", status_code)
                .field("body_excerpt", &"[REDACTED]")
                .finish(),
            Self::InvalidResponse(_) => formatter
                .debug_tuple("InvalidResponse")
                .field(&"[REDACTED]")
                .finish(),
            Self::Transport(_) => formatter
                .debug_tuple("Transport")
                .field(&"[REDACTED]")
                .finish(),
            Self::Storage(_) => formatter
                .debug_tuple("Storage")
                .field(&"[REDACTED]")
                .finish(),
            Self::EncryptionUnavailable => formatter.write_str("EncryptionUnavailable"),
        }
    }
}

/// Builds an error excerpt that is safe to persist or render in diagnostics.
///
/// Structured responses retain non-sensitive provider error codes/messages so
/// callers can still classify `invalid_grant` and similar failures. Secret
/// fields and secret-shaped values are removed before the size bound is
/// applied. Unstructured bodies containing credential markers are replaced as
/// a whole because their token boundaries cannot be determined reliably.
pub fn redacted_oauth_error_body_excerpt(body: &str) -> String {
    let body = body.trim();
    if body.is_empty() {
        return "-".to_string();
    }

    if let Ok(mut value) = serde_json::from_str::<Value>(body) {
        redact_oauth_error_json(&mut value);
        return value
            .to_string()
            .chars()
            .take(OAUTH_ERROR_BODY_EXCERPT_CHARS)
            .collect();
    }

    if unstructured_body_may_contain_secret(body) {
        "[REDACTED upstream OAuth error body]".to_string()
    } else {
        body.chars().take(OAUTH_ERROR_BODY_EXCERPT_CHARS).collect()
    }
}

fn redact_oauth_error_json(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if oauth_error_key_is_sensitive(key) {
                    *value = json!("[REDACTED]");
                } else {
                    redact_oauth_error_json(value);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                redact_oauth_error_json(item);
            }
        }
        Value::String(text) => {
            if oauth_error_value_is_safe_classification_code(text) {
                // Keep a small allowlist of non-secret provider error codes for classification.
            } else if oauth_error_value_looks_secret(text)
                || unstructured_body_may_contain_secret(text)
            {
                *text = "[REDACTED]".to_string();
            } else {
                *text = redact_urls_in_text(text);
            }
        }
        _ => {}
    }
}

fn oauth_error_key_is_sensitive(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    normalized.contains("token")
        || normalized.contains("apikey")
        || normalized.contains("password")
        || normalized.contains("authorization")
        || normalized.contains("secret")
        || normalized.contains("clientsecret")
        || normalized.contains("privatekey")
        || normalized.contains("assertion")
        || normalized.contains("credential")
        || normalized.contains("cookie")
        || normalized.contains("pkce")
        || normalized.contains("verifier")
        || normalized == "sessionkey"
}

fn oauth_error_value_looks_secret(value: &str) -> bool {
    let value = value.trim();
    value.starts_with("Bearer ")
        || value.starts_with("bearer ")
        || value.starts_with("sk-")
        || value.starts_with("sess-")
        || value.starts_with("devin-session-token$")
        || value.starts_with("ott$")
        || value.starts_with("auth1_")
        || (value.len() > 80
            && value.split('.').count() == 3
            && value.split('.').all(|segment| {
                !segment.is_empty()
                    && segment.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'=')
                    })
            }))
}

fn oauth_error_value_is_safe_classification_code(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "refresh_token_reused" | "refresh_token_expired" | "invalid_refresh_token"
    )
}

/// Redact dynamic OAuth error details before they reach `Display` consumers.
/// Error details frequently originate in HTTP clients and may contain a full
/// request URL or an upstream response body.
fn redact_oauth_error_detail(detail: &str) -> String {
    let excerpt = redacted_oauth_error_body_excerpt(detail);
    redact_urls_in_text(&excerpt)
        .chars()
        .take(OAUTH_ERROR_BODY_EXCERPT_CHARS)
        .collect()
}

fn redact_urls_in_text(text: &str) -> String {
    const URL_SCHEMES: [&str; 4] = ["http://", "https://", "ws://", "wss://"];
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;

    while cursor < text.len() {
        let Some((relative_start, scheme)) = URL_SCHEMES
            .iter()
            .filter_map(|scheme| text[cursor..].find(scheme).map(|start| (start, *scheme)))
            .min_by_key(|(start, _)| *start)
        else {
            output.push_str(&text[cursor..]);
            break;
        };

        let start = cursor + relative_start;
        output.push_str(&text[cursor..start]);
        let token_end = text[start..]
            .find(char::is_whitespace)
            .map(|offset| start + offset)
            .unwrap_or(text.len());
        let token = &text[start..token_end];
        let (url_token, suffix) = trim_url_suffix(token);
        if url_token.starts_with(scheme) {
            if url::Url::parse(url_token).is_ok() {
                output.push_str(&redact_url_for_debug(url_token));
            } else {
                output.push_str("[REDACTED URL]");
            }
            output.push_str(suffix);
        } else {
            output.push_str(token);
        }
        cursor = token_end;
    }

    output
}

fn trim_url_suffix(token: &str) -> (&str, &str) {
    let mut end = token.len();
    while end > 0 {
        let Some(ch) = token[..end].chars().next_back() else {
            break;
        };
        if matches!(
            ch,
            '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}' | '\'' | '"'
        ) {
            end -= ch.len_utf8();
        } else {
            break;
        }
    }
    (&token[..end], &token[end..])
}

fn unstructured_body_may_contain_secret(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase();
    [
        "access_token",
        "refresh_token",
        "id_token",
        "api_key",
        "apikey",
        "authorization:",
        "authorization=",
        "client_secret",
        "secret=",
        "secret:",
        "password=",
        "password:",
        "assertion=",
        "session_token",
        "sessiontoken",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
        || oauth_error_value_looks_secret(value)
}

impl OAuthError {
    pub fn invalid_request(detail: impl Into<String>) -> Self {
        Self::InvalidRequest(detail.into())
    }

    pub fn invalid_response(detail: impl Into<String>) -> Self {
        Self::InvalidResponse(detail.into())
    }

    pub fn transport(detail: impl Into<String>) -> Self {
        Self::Transport(detail.into())
    }
}

#[cfg(test)]
mod tests {
    use super::{redacted_oauth_error_body_excerpt, OAuthError};

    #[test]
    fn oauth_error_body_excerpt_preserves_classification_and_redacts_secrets() {
        let excerpt = redacted_oauth_error_body_excerpt(
            r#"{
                "error": {
                    "code": "invalid_grant",
                    "message": "refresh token expired",
                    "refresh_token": "refresh-body-canary",
                    "nested": {"clientSecret": "client-secret-canary"}
                },
                "accessToken": "access-token-canary"
            }"#,
        );

        assert!(excerpt.contains("invalid_grant"));
        assert!(excerpt.contains("refresh token expired"));
        assert!(!excerpt.contains("refresh-body-canary"));
        assert!(!excerpt.contains("client-secret-canary"));
        assert!(!excerpt.contains("access-token-canary"));
        assert!(excerpt.contains("[REDACTED]"));
    }

    #[test]
    fn oauth_error_debug_and_display_do_not_render_upstream_body() {
        let error = OAuthError::HttpStatus {
            status_code: 401,
            body_excerpt: "authorization=Bearer oauth-error-canary".to_string(),
        };

        let debug = format!("{error:?}");
        let display = error.to_string();
        assert!(!debug.contains("oauth-error-canary"));
        assert!(!display.contains("oauth-error-canary"));
        assert!(debug.contains("[REDACTED]"));
        assert_eq!(display, "oauth provider returned HTTP 401");
    }

    #[test]
    fn unstructured_oauth_error_with_secret_markers_is_replaced() {
        let excerpt = redacted_oauth_error_body_excerpt(
            "invalid request: refresh_token=plain-text-refresh-canary",
        );
        assert_eq!(excerpt, "[REDACTED upstream OAuth error body]");
    }

    #[test]
    fn oauth_error_body_excerpt_preserves_long_refresh_rotation_message() {
        let body = r#"{"error":{"message":"Your refresh token has already been used to generate a new access token. Please try signing in again.","type":"invalid_request_error","param":null,"code":"refresh_token_reused"}}"#;
        let excerpt = redacted_oauth_error_body_excerpt(body);
        assert!(excerpt.contains("already been used to generate a new access token"));
        assert!(excerpt.contains("refresh_token_reused"));
    }

    #[test]
    fn dynamic_oauth_error_display_redacts_credentials_and_url_queries() {
        let detail =
            "apiKey=sk-secret token=secret-token https://user:pass@example.test?q=url-secret";
        for error in [
            OAuthError::UnsupportedProvider(detail.to_string()),
            OAuthError::InvalidRequest(detail.to_string()),
            OAuthError::InvalidResponse(detail.to_string()),
            OAuthError::Transport(detail.to_string()),
            OAuthError::Storage(detail.to_string()),
        ] {
            let display = error.to_string();
            for secret in ["sk-secret", "secret-token", "user", "pass", "url-secret"] {
                assert!(
                    !display.contains(secret),
                    "display leaked {secret}: {display}"
                );
            }
        }
    }

    #[test]
    fn dynamic_oauth_error_display_redacts_standalone_url() {
        let error = OAuthError::invalid_response(
            "upstream request failed at https://user:pass@example.test/path?code=secret",
        );
        let display = error.to_string();
        assert!(!display.contains("user"));
        assert!(!display.contains("pass"));
        assert!(!display.contains("code=secret"));
        assert!(display.contains("https://example.test/path"));
    }
}
