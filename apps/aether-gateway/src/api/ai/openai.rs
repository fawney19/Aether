pub(crate) fn normalized_signature(api_format: &str) -> Option<&'static str> {
    match api_format {
        "openai:chat" => Some("openai:chat"),
        "openai:cli" => Some("openai:cli"),
        "openai:compact" => Some("openai:compact"),
        "openai:image" => Some("openai:image"),
        "openai:video" => Some("openai:video"),
        _ => None,
    }
}

pub(crate) fn local_path(api_format: &str) -> Option<&'static str> {
    match api_format {
        "openai" | "openai:chat" => Some("/v1/chat/completions"),
        "openai:cli" => Some("/v1/responses"),
        "openai:compact" => Some("/v1/responses/compact"),
        "openai:image" => Some("/v1/images/generations"),
        "openai:video" => Some("/v1/videos"),
        _ => None,
    }
}
