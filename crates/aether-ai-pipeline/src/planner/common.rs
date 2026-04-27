use base64::Engine as _;

pub fn parse_direct_request_body(
    is_json_request: bool,
    body_bytes: &[u8],
) -> Option<(serde_json::Value, Option<String>)> {
    if is_json_request {
        if body_bytes.is_empty() {
            Some((serde_json::json!({}), None))
        } else {
            serde_json::from_slice::<serde_json::Value>(body_bytes)
                .ok()
                .map(|value| (value, None))
        }
    } else {
        Some((
            serde_json::json!({}),
            (!body_bytes.is_empty())
                .then(|| base64::engine::general_purpose::STANDARD.encode(body_bytes)),
        ))
    }
}

pub fn force_upstream_streaming_for_provider(
    provider_type: &str,
    provider_api_format: &str,
) -> bool {
    provider_type.trim().eq_ignore_ascii_case("codex")
        && aether_ai_formats::is_openai_responses_format(provider_api_format)
}

#[cfg(test)]
mod tests {
    use super::{force_upstream_streaming_for_provider, parse_direct_request_body};

    #[test]
    fn parses_empty_json_body_as_empty_object() {
        assert_eq!(
            parse_direct_request_body(true, b""),
            Some((serde_json::json!({}), None))
        );
    }

    #[test]
    fn rejects_invalid_json_body() {
        assert_eq!(parse_direct_request_body(true, b"{invalid"), None);
    }

    #[test]
    fn encodes_non_json_body_as_base64() {
        assert_eq!(
            parse_direct_request_body(false, b"hello"),
            Some((serde_json::json!({}), Some("aGVsbG8=".to_string())))
        );
    }

    #[test]
    fn forces_streaming_for_codex_openai_responses() {
        assert!(force_upstream_streaming_for_provider(
            "codex",
            "openai:responses"
        ));
        assert!(force_upstream_streaming_for_provider("codex", "openai:cli"));
    }

    #[test]
    fn does_not_force_streaming_for_compact_or_other_provider_types() {
        assert!(!force_upstream_streaming_for_provider(
            "codex",
            "openai:compact"
        ));
        assert!(!force_upstream_streaming_for_provider(
            "openai",
            "openai:cli"
        ));
    }
}
