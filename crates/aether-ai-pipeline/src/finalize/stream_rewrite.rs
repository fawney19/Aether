use serde_json::Value;

use crate::adaptation::surfaces::{
    provider_adaptation_should_unwrap_stream_envelope, KIRO_ENVELOPE_NAME,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalizeStreamRewriteMode {
    EnvelopeUnwrap,
    OpenAiImage,
    Standard,
    KiroToClaudeCli,
    KiroToClaudeCliThenStandard,
}

pub fn resolve_finalize_stream_rewrite_mode(
    report_context: &Value,
) -> Option<FinalizeStreamRewriteMode> {
    let needs_conversion = report_context
        .get("needs_conversion")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let envelope_name = report_context
        .get("envelope_name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let provider_api_format = report_context
        .get("provider_api_format")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let client_api_format = report_context
        .get("client_api_format")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();

    if needs_conversion
        && envelope_name.eq_ignore_ascii_case(KIRO_ENVELOPE_NAME)
        && provider_api_format == "claude:cli"
    {
        return supports_standard_stream_rewrite(
            provider_api_format.as_str(),
            client_api_format.as_str(),
        )
        .then_some(FinalizeStreamRewriteMode::KiroToClaudeCliThenStandard);
    }

    if needs_conversion {
        return supports_standard_stream_rewrite(
            provider_api_format.as_str(),
            client_api_format.as_str(),
        )
        .then_some(FinalizeStreamRewriteMode::Standard);
    }

    if provider_api_format == "openai:image" && client_api_format == "openai:image" {
        return Some(FinalizeStreamRewriteMode::OpenAiImage);
    }

    if envelope_name.eq_ignore_ascii_case(KIRO_ENVELOPE_NAME) {
        return (provider_api_format == "claude:cli" && client_api_format == "claude:cli")
            .then_some(FinalizeStreamRewriteMode::KiroToClaudeCli);
    }

    (provider_api_format == client_api_format
        && provider_adaptation_should_unwrap_stream_envelope(
            envelope_name.as_str(),
            provider_api_format.as_str(),
        ))
    .then_some(FinalizeStreamRewriteMode::EnvelopeUnwrap)
}

fn supports_standard_stream_rewrite(provider_api_format: &str, client_api_format: &str) -> bool {
    is_standard_provider_api_format(provider_api_format)
        && (is_standard_chat_client_api_format(client_api_format)
            || is_standard_cli_client_api_format(client_api_format))
}

fn is_standard_provider_api_format(api_format: &str) -> bool {
    matches!(
        aether_ai_formats::normalize_legacy_openai_format_alias(api_format).as_str(),
        "openai:chat"
            | "openai:responses"
            | "openai:responses:compact"
            | "claude:chat"
            | "claude:cli"
            | "gemini:chat"
            | "gemini:cli"
    )
}

fn is_standard_chat_client_api_format(api_format: &str) -> bool {
    matches!(api_format, "openai:chat" | "claude:chat" | "gemini:chat")
}

fn is_standard_cli_client_api_format(api_format: &str) -> bool {
    matches!(
        aether_ai_formats::normalize_legacy_openai_format_alias(api_format).as_str(),
        "openai:responses" | "openai:responses:compact" | "claude:cli" | "gemini:cli"
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{resolve_finalize_stream_rewrite_mode, FinalizeStreamRewriteMode};

    #[test]
    fn resolves_standard_mode_for_cross_format_standard_streams() {
        let report_context = json!({
            "provider_api_format": "claude:chat",
            "client_api_format": "openai:chat",
            "needs_conversion": true,
        });
        assert_eq!(
            resolve_finalize_stream_rewrite_mode(&report_context),
            Some(FinalizeStreamRewriteMode::Standard)
        );
    }

    #[test]
    fn resolves_envelope_unwrap_for_same_format_private_envelopes() {
        let report_context = json!({
            "provider_api_format": "gemini:cli",
            "client_api_format": "gemini:cli",
            "envelope_name": "antigravity:v1internal",
            "needs_conversion": false,
        });
        assert_eq!(
            resolve_finalize_stream_rewrite_mode(&report_context),
            Some(FinalizeStreamRewriteMode::EnvelopeUnwrap)
        );
    }

    #[test]
    fn resolves_kiro_same_format_streams_to_kiro_mode() {
        let report_context = json!({
            "provider_api_format": "claude:cli",
            "client_api_format": "claude:cli",
            "envelope_name": "kiro:generateAssistantResponse",
            "needs_conversion": false,
        });
        assert_eq!(
            resolve_finalize_stream_rewrite_mode(&report_context),
            Some(FinalizeStreamRewriteMode::KiroToClaudeCli)
        );
    }

    #[test]
    fn rejects_unsupported_non_conversion_streams() {
        let report_context = json!({
            "provider_api_format": "openai:chat",
            "client_api_format": "openai:chat",
            "needs_conversion": false,
        });
        assert_eq!(resolve_finalize_stream_rewrite_mode(&report_context), None);
    }

    #[test]
    fn resolves_openai_image_mode_for_same_format_image_streams() {
        let report_context = json!({
            "provider_api_format": "openai:image",
            "client_api_format": "openai:image",
            "needs_conversion": false,
        });
        assert_eq!(
            resolve_finalize_stream_rewrite_mode(&report_context),
            Some(FinalizeStreamRewriteMode::OpenAiImage)
        );
    }
}
