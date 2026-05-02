pub(crate) fn normalized_signature(api_format: &str) -> Option<&'static str> {
    match crate::ai_serving::normalize_api_format_alias(api_format).as_str() {
        "gemini:generate_content" => Some("gemini:generate_content"),
        "gemini:video" => Some("gemini:video"),
        "gemini:files" => Some("gemini:files"),
        _ => None,
    }
}

pub(crate) fn local_path(api_format: &str) -> Option<&'static str> {
    match crate::ai_serving::normalize_api_format_alias(api_format).as_str() {
        "gemini" | "gemini:generate_content" => Some("/v1beta/models/{model}:{action}"),
        "gemini:video" => Some("/v1beta/models/{model}:predictLongRunning"),
        "gemini:files" => Some("/v1beta/files"),
        _ => None,
    }
}
