pub(crate) fn normalized_signature(api_format: &str) -> Option<&'static str> {
    match crate::ai_serving::normalize_api_format_alias(api_format).as_str() {
        "openai:chat" => Some("openai:chat"),
        "openai:embedding" => Some("openai:embedding"),
        "openai:rerank" => Some("openai:rerank"),
        "openai:responses" => Some("openai:responses"),
        "openai:responses:compact" => Some("openai:responses:compact"),
        "openai:search" => Some("openai:search"),
        "openai:image" => Some("openai:image"),
        "openai:video" => Some("openai:video"),
        _ => None,
    }
}

pub(crate) fn local_path(api_format: &str) -> Option<&'static str> {
    match crate::ai_serving::normalize_api_format_alias(api_format).as_str() {
        "openai" | "openai:chat" => Some("/v1/chat/completions"),
        "openai:embedding" => Some("/v1/embeddings"),
        "openai:rerank" => Some("/v1/rerank"),
        "openai:responses" => Some("/v1/responses"),
        "openai:responses:compact" => Some("/v1/responses/compact"),
        "openai:search" => Some("/v1/alpha/search"),
        "openai:image" => Some("/v1/images/generations"),
        "openai:video" => Some("/v1/videos"),
        _ => None,
    }
}
