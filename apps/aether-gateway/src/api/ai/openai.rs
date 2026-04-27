pub(crate) fn normalized_signature(api_format: &str) -> Option<&'static str> {
    match crate::ai_pipeline::normalize_legacy_openai_format_alias(api_format).as_str() {
        "openai:chat" => Some("openai:chat"),
        "openai:responses" => Some("openai:responses"),
        "openai:responses:compact" => Some("openai:responses:compact"),
        "openai:image" => Some("openai:image"),
        "openai:video" => Some("openai:video"),
        _ => None,
    }
}

pub(crate) fn local_path(api_format: &str) -> Option<&'static str> {
    match crate::ai_pipeline::normalize_legacy_openai_format_alias(api_format).as_str() {
        "openai" | "openai:chat" => Some("/v1/chat/completions"),
        "openai:responses" => Some("/v1/responses"),
        "openai:responses:compact" => Some("/v1/responses/compact"),
        "openai:image" => Some("/v1/images/generations"),
        "openai:video" => Some("/v1/videos"),
        _ => None,
    }
}
