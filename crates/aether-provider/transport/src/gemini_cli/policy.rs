pub fn gemini_cli_v1internal_requires_upstream_streaming(
    provider_api_format: &str,
    client_requires_streaming: bool,
) -> bool {
    client_requires_streaming
        && aether_ai_formats::normalize_api_format_alias(provider_api_format)
            == "gemini:generate_content"
}

#[cfg(test)]
mod tests {
    use super::gemini_cli_v1internal_requires_upstream_streaming;

    #[test]
    fn v1internal_generate_content_requires_upstream_streaming_for_stream_clients() {
        assert!(gemini_cli_v1internal_requires_upstream_streaming(
            "gemini:generate_content",
            true,
        ));
        assert!(!gemini_cli_v1internal_requires_upstream_streaming(
            "gemini:generate_content",
            false,
        ));
        assert!(!gemini_cli_v1internal_requires_upstream_streaming(
            "openai:chat",
            true,
        ));
    }
}
